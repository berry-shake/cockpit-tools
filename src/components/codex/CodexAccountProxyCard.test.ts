import assert from 'node:assert/strict';
import test from 'node:test';
import { loadHookModule } from '../../../tests/helpers/reactHookHarness';
import { canUseCodexAccountProxy } from '../../utils/codexAccountProxy';
import { codexProxyCardPresentation, type CodexProxyDisplay } from '../../utils/codexProxyDisplay';
import { proxyRuntimeLabelKey } from '../../utils/codexProxyPreview';
import type { CodexAccount } from '../../types/codex';

function elements(value: any): any[] {
  if (!value || typeof value !== 'object') return [];
  if (Array.isArray(value)) return value.flatMap(elements);
  return [value, ...elements(value.props?.children)];
}

test('all display modes retain the original proxy action and supplementary cards open the same account', () => {
  const opened: string[] = [];
  let display: CodexProxyDisplay = 'compact';
  const account = { id: 'account', egress_proxy: { protocol: 'resource', name: 'Auto' } } as CodexAccount;
  const imports = {
    'react-i18next': { useTranslation: () => ({ t: (key: string) => key }) },
    '../../hooks/useCodexProxyDisplay': { useCodexProxyDisplay: () => display },
    '../../utils/codexAccountProxy': { canUseCodexAccountProxy, requestCodexAccountProxy: (id: string) => opened.push(id) },
    '../../utils/codexProxyPresentation': { proxySummary: () => 'Auto' },
    '../../utils/codexProxyDisplay': { codexProxyCardPresentation },
    '../../utils/codexProxyPreview': { proxyRuntimeLabelKey },
    './useCodexProxyCardData': { useCodexProxyCardData: () => ({ ref: { current: null }, status: {
      account: 'running', sidecar: 'running', desktop: 'running',
      accountPort: 45101, sidecarPort: 45101, desktopPort: 45101,
      accountNode: 'Current leaf', sidecarNode: 'Current leaf', desktopNode: 'Current leaf',
      accountSelection: { name: 'Current leaf', delayMs: 42, checkedAt: 1234 },
      desktopEntry: { state: 'listening', port: 45101, requestCount: 1, lastRequestState: 'forwarded', lastError: null },
    }, request: null, loading: false, statusError: false, requestsError: false }) },
  };
  const button = loadHookModule(new URL('./CodexAccountProxyButton.tsx', import.meta.url), imports);
  const card = loadHookModule(new URL('./CodexAccountProxyCard.tsx', import.meta.url), imports);
  let stopped = 0;
  for (display of ['compact', 'summary', 'detailed'] as const) {
    const action = button.render(() => button.exports.CodexAccountProxyButton({ account }));
    assert.equal(action.type, 'button');
    assert.ok(action.props.children.includes('codex.proxy.action'));
    action.props.onClick({ stopPropagation() { stopped++; } });
    for (const placement of ['summary', 'detailed']) {
      const content = card.render(() => card.exports.CodexAccountProxyCard({ account, placement }));
      if (display !== placement) { assert.equal(content, null); continue; }
      const tree = card.render(() => content.type(content.props));
      const open = elements(tree).find((node) => node.type === 'button');
      open.props.onClick({ stopPropagation() { stopped++; } });
      assert.equal(elements(tree).filter((node) => node.props?.className === 'codex-account-proxy-card-node').length, 1);
      assert.equal(elements(tree).filter((node) => node.props?.className === 'codex-account-proxy-card-channel').length, display === 'detailed' ? 1 : 0);
    }
  }
  assert.deepEqual(opened, Array(5).fill('account'));
  assert.equal(stopped, 5);
  const ineligible = { ...account, agent_identity: true };
  assert.equal(button.render(() => button.exports.CodexAccountProxyButton({ account: ineligible })), null);
  assert.equal(card.render(() => card.exports.CodexAccountProxyCard({ account: ineligible, placement: 'detailed' })), null);
});
