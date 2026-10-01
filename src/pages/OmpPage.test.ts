import assert from 'node:assert/strict';
import test from 'node:test';
import { deferred, loadHookModule, settlePromises } from '../../tests/helpers/reactHookHarness';
import type { OmpState } from '../services/ompService';

interface Element { type: unknown; props: Record<string, unknown> }
function nodes(tree: unknown): Element[] {
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  if (!tree || typeof tree !== 'object' || !('props' in tree)) return [];
  const element = tree as Element;
  return [element, ...nodes(element.props.children)];
}
function text(tree: unknown): string {
  if (typeof tree === 'string') return tree;
  if (Array.isArray(tree)) return tree.map(text).join('');
  if (tree && typeof tree === 'object' && 'props' in tree) return text((tree as Element).props.children);
  return '';
}
function click(element: Element) { (element.props.onClick as () => void)(); }
function change(element: Element, value: string) { (element.props.onChange as (e: { target: { value: string } }) => void)({ target: { value } }); }

function harness() {
  const state: OmpState = { executable: '/fixture/omp', databasePath: '/fixture/.omp/agent/agent.db', warning: null, loginSupported: true, accounts: [
    { id: 1, identity: 'one', provider: 'openai-codex', credentialType: 'oauth', email: 'one@example.invalid', accountId: null, orgName: 'work', status: 'pool', inNativeStore: true },
    { id: 2, identity: 'two', provider: 'openai-codex', credentialType: 'oauth', email: 'two@example.invalid', accountId: null, orgName: null, status: 'pool', inNativeStore: true },
    { id: 3, identity: 'three', provider: 'anthropic', credentialType: 'oauth', email: 'three@example.invalid', accountId: null, orgName: null, status: 'current', inNativeStore: true },
    { id: 4, identity: 'four', provider: 'anthropic', credentialType: 'oauth', email: 'four@example.invalid', accountId: null, orgName: null, status: 'trash', inNativeStore: true },
  ] };
  const calls: { kind: string; id?: number; identity?: string; action?: string; executable?: string }[] = [];
  const pending = deferred<void>();
  let readError = false;
  let modal: { actions: { onClick?: () => Promise<void> }[] } | null = null;
  const h = loadHookModule(new URL('./OmpPage.tsx', import.meta.url), {
    '../components/SingleSelectDropdown': { SingleSelectDropdown: 'SingleSelectDropdown' },
    '../components/icons/OmpIcon': { OmpIcon: 'OmpIcon' },
    'react-i18next': { useTranslation: () => ({ t: (key: string) => key }) },
    '@tauri-apps/plugin-dialog': { open: async () => null },
    '../hooks/useGlobalModal': { useGlobalModal: () => ({ showModal(value: typeof modal) { modal = value; } }) },
    '../services/ompService': {
      async getOmpState() { if (readError) throw new Error('fixture read failed'); return structuredClone(state); },
      async actOnOmpAccount(id: number, identity: string, action: string) {
        calls.push({ kind: 'account', id, identity, action }); await pending.promise;
        const account = state.accounts.find((a) => a.id === id)!;
        account.status = action === 'switch' ? 'current' : action === 'trash' ? 'trash' : 'standby';
      },
      async loginOmp(executable: string) { calls.push({ kind: 'login', executable }); await pending.promise; },
    },
  });
  h.render(() => h.exports.OmpPage());
  const card = (email: string) => nodes(h.flush()).find((n) => n.type === 'article' && text(n).includes(email));
  const button = (key: string, email?: string) => {
    const scope = email ? card(email) : h.flush();
    const found = nodes(scope).find((n) => n.type === 'button' && (text(n).trim() === key || n.props['aria-label'] === key || text(n).startsWith(key)));
    assert.ok(found, `missing ${key}`); return found;
  };
  const input = (key: string) => {
    const found = nodes(h.flush()).find((n) => ['input', 'select'].includes(String(n.type)) && (n.props.id === key || n.props['aria-label'] === key));
    assert.ok(found); return found;
  };
  const filter = () => {
    const found = nodes(h.flush()).find((n) => n.type === 'SingleSelectDropdown');
    assert.ok(found); return found;
  };
  return { h, state, calls, pending, card, button, input, filter,
    confirm() { assert.ok(modal); return modal.actions[1].onClick!(); },
    failRead() { readError = true; },
  };
}

test('switch requires confirmation, passes account identity and suppresses duplicate mutations', async () => {
  const h = harness(); await settlePromises();
  click(h.button('omp.account.switch', 'one@example.invalid'));
  assert.equal(h.calls.length, 0);
  void h.confirm(); void h.confirm();
  assert.deepEqual(h.calls, [{ kind: 'account', id: 1, identity: 'one', action: 'switch' }]);
  assert.equal(h.button('omp.account.switch', 'two@example.invalid').props.disabled, true);
  h.pending.resolve(); await settlePromises();
  assert.equal(h.button('omp.account.switch', 'one@example.invalid').props.disabled, true);
  assert.match(text(h.card('one@example.invalid')), /omp.account.providerSelected/);
  assert.match(text(h.h.flush()), /omp.account.switched/); h.h.unmount();
});

test('failed account writes remain visible without a success notice or changed current marker', async () => {
  const h = harness(); await settlePromises();
  click(h.button('omp.account.switch', 'two@example.invalid')); void h.confirm();
  h.pending.reject(new Error('fixture refresh lease busy')); await settlePromises();
  assert.match(text(h.h.flush()), /fixture refresh lease busy/);
  assert.doesNotMatch(text(h.h.flush()), /omp.account.switched/);
  assert.equal(h.button('omp.account.switch', 'two@example.invalid').props.disabled, false); h.h.unmount();
});

test('trash is hidden from the default account list and restore is a distinct confirmed action', async () => {
  const h = harness(); await settlePromises(); assert.equal(h.card('four@example.invalid'), undefined);
  click(h.button('omp.account.recycleBin')); assert.ok(h.card('four@example.invalid'));
  click(h.button('omp.account.restore', 'four@example.invalid')); void h.confirm();
  assert.equal(h.calls[0].action, 'restore'); h.pending.resolve(); await settlePromises();
  assert.equal(h.card('four@example.invalid'), undefined); click(h.button('omp.account.all'));
  assert.ok(h.card('four@example.invalid')); assert.match(text(h.card('four@example.invalid')), /omp.account.standby/); h.h.unmount();
});

test('search, provider filtering and identity masking operate on account cards, not profiles', async () => {
  const h = harness(); await settlePromises();
  change(h.input('omp.account.search'), 'two@'); assert.ok(h.card('two@example.invalid')); assert.equal(h.card('one@example.invalid'), undefined);
  change(h.input('omp.account.search'), '');
  (h.filter().props.onChange as (value: string) => void)('anthropic');
  assert.ok(h.card('three@example.invalid')); assert.equal(h.card('one@example.invalid'), undefined);
  click(h.button('omp.account.hideIdentity')); assert.doesNotMatch(text(h.h.flush()), /three@example.invalid/);
  assert.ok(nodes(h.h.flush()).every((n) => n.props.title !== 'three@example.invalid')); h.h.unmount();
});

test('default layout is compact and separates selection by provider without global current labels', async () => {
  const h = harness(); h.state.accounts[0].status = 'current'; h.state.accounts[1].status = 'standby';
  await settlePromises(); click(h.button('common.refresh')); await settlePromises();
  const tree = h.h.flush();
  assert.ok(nodes(tree).some((n) => n.props.className === 'omp-account-groups list'));
  const groups = nodes(tree).filter((n) => n.type === 'section' && n.props.className === 'omp-provider-group');
  assert.equal(groups.length, 2);
  assert.ok(groups.every((group) => nodes(group).filter((n) => n.type === 'article').length > 0));
  assert.equal(text(tree).split('omp.account.providerSelected').length - 1, 2);
  assert.doesNotMatch(text(tree), /omp.account.current/);
  assert.ok(nodes(tree).some((n) => n.type === 'OmpIcon'));
  assert.ok(nodes(tree).every((n) => n.type !== 'select'));
  click(h.button('omp.account.cardView'));
  assert.ok(nodes(h.h.flush()).some((n) => n.props.className === 'omp-account-groups cards'));
  assert.deepEqual(h.calls, []); h.h.unmount();
});

test('custom provider filter can be cleared and does not render empty provider groups', async () => {
  const h = harness(); await settlePromises();
  const choose = (value: string) => (h.filter().props.onChange as (value: string) => void)(value);
  assert.equal(h.filter().props.ariaLabel, 'omp.account.providerFilter');
  choose('openai-codex');
  assert.equal(nodes(h.h.flush()).filter((n) => n.props.className === 'omp-provider-group').length, 1);
  assert.equal(h.card('three@example.invalid'), undefined);
  choose(''); assert.ok(h.card('three@example.invalid'));
  change(h.input('omp.account.search'), 'nothing matches');
  assert.equal(nodes(h.h.flush()).filter((n) => n.props.className === 'omp-provider-group').length, 0);
  assert.match(text(h.h.flush()), /omp.account.noMatches/);
  assert.deepEqual(h.calls, []); h.h.unmount();
});

test('native login needs only executable, never a profile or project directory, and does not claim completion', async () => {
  const h = harness(); await settlePromises(); click(h.button('common.addAccount'));
  click(h.button('omp.account.nativeLogin'));
  assert.deepEqual(h.calls, [{ kind: 'login', executable: '/fixture/omp' }]);
  h.pending.resolve(); await settlePromises();
  assert.match(text(h.h.flush()), /omp.account.loginSent/); h.h.unmount();
});

test('a committed mutation with a failed reread explicitly asks the user to refresh', async () => {
  const h = harness(); await settlePromises(); click(h.button('omp.account.switch', 'one@example.invalid'));
  void h.confirm(); h.failRead(); h.pending.resolve(); await settlePromises();
  assert.match(text(h.h.flush()), /omp.account.reloadAfterWrite/); h.h.unmount();
});

test('incompatible native stores disable account mutations without hiding their warning', async () => {
  const h = harness(); h.state.warning = 'fixture unsupported schema'; await settlePromises();
  click(h.button('common.refresh')); await settlePromises();
  assert.equal(h.button('omp.account.switch', 'one@example.invalid').props.disabled, true);
  assert.match(text(h.h.flush()), /fixture unsupported schema/); h.h.unmount();
});
