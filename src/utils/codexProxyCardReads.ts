import { singleFlightRead } from './codexProxyPreview';

export const PROXY_CARD_REFRESH_MS = 20_000;
const READ_TIMEOUT_MS = 10_000;

/** Keep permits until the native operation settles, including after a UI timeout. */
export function createProxyCardReadPool(limit = 4, timeoutMs = READ_TIMEOUT_MS) {
  let active = 0;
  const queue: Array<() => void> = [];
  const drain = () => {
    while (active < limit && queue.length) queue.shift()!();
  };
  return <T>(read: () => Promise<T>, isActive: () => boolean): Promise<T> => new Promise((resolve, reject) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const start = () => {
      clearTimeout(timer);
      if (!isActive()) { reject(new Error('PROXY_CARD_INACTIVE')); return; }
      active++;
      void Promise.resolve().then(read).then(resolve, reject).finally(() => { active--; drain(); });
    };
    queue.push(start);
    timer = setTimeout(() => {
      const index = queue.indexOf(start);
      if (index >= 0) { queue.splice(index, 1); reject(new Error('PROXY_CARD_QUEUE_TIMEOUT')); }
    }, timeoutMs);
    drain();
  });
}

export interface ProxyCardReadResult<T> { value: T | null; error: boolean }

/** Shared snapshot cache; failures preserve the last successful value. */
export function createProxyCardReadCache<T>(
  read: (accountId: string) => Promise<T>,
  pool: ReturnType<typeof createProxyCardReadPool>,
  timeoutMs = READ_TIMEOUT_MS,
) {
  const snapshots = new Map<string, ProxyCardReadResult<T> & { checkedAt: number }>();
  const revisions = new Map<string, number>();
  const consumers = new Map<string, Set<() => boolean>>();
  const nativeReads = singleFlightRead((key) => {
    const accountId = JSON.parse(JSON.parse(key)[0])[0] as string;
    return pool(() => read(accountId), () => [...(consumers.get(key) ?? [])].some((check) => check()));
  }, timeoutMs);
  return {
    invalidateAccount(accountId: string) {
      revisions.set(accountId, (revisions.get(accountId) ?? 0) + 1);
      for (const key of snapshots.keys()) {
        if (JSON.parse(key)[0] === accountId) snapshots.delete(key);
      }
    },
    has(key: string): boolean { return snapshots.has(key); },
    peek(key: string): ProxyCardReadResult<T> {
      return snapshots.get(key) ?? { value: null, error: false };
    },
    async read(key: string, isActive: () => boolean, force = false): Promise<ProxyCardReadResult<T>> {
      const accountId = JSON.parse(key)[0] as string;
      const revision = revisions.get(accountId) ?? 0;
      const nativeKey = JSON.stringify([key, revision]);
      const current = () => isActive() && revision === (revisions.get(accountId) ?? 0);
      const cached = snapshots.get(key);
      if (!force && cached && Date.now() - cached.checkedAt < PROXY_CARD_REFRESH_MS) return cached;
      const checks = consumers.get(nativeKey) ?? new Set<() => boolean>();
      checks.add(current);
      consumers.set(nativeKey, checks);
      const checkedAt = Date.now();
      try {
        const value = await nativeReads(nativeKey);
        if (!current()) return this.peek(key);
        const next = { value, error: false, checkedAt };
        snapshots.delete(key);
        snapshots.set(key, next);
        // Do not retain every account/binding ever visited for the entire session.
        if (snapshots.size > 256) snapshots.delete(snapshots.keys().next().value!);
        return next;
      } catch {
        if (!current()) return this.peek(key);
        const next = { value: snapshots.get(key)?.value ?? null, error: true, checkedAt };
        snapshots.delete(key);
        snapshots.set(key, next);
        if (snapshots.size > 256) snapshots.delete(snapshots.keys().next().value!);
        return next;
      } finally {
        checks.delete(current);
        if (!checks.size) consumers.delete(nativeKey);
      }
    },
  };
}
