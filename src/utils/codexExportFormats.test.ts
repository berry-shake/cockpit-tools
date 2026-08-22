import assert from 'node:assert/strict';
import test from 'node:test';
import type { CodexAccount } from '../types/codex.ts';
import {
  buildCodexExportContent,
  hasCodexExportAgentIdentity,
  transformCodexExportJson,
} from './codexExportFormats.ts';

function agentIdentityAccount(): CodexAccount {
  return {
    id: 'codex-agent-fixture',
    email: 'fixture@example.com',
    auth_mode: 'oauth',
    account_id: 'account-fixture',
    user_id: 'user-fixture',
    plan_type: 'plus',
    agent_identity: {
      agent_runtime_id: 'runtime-fixture',
      agent_private_key: 'private-key-fixture',
      task_id: 'task-fixture',
      account_id: 'account-fixture',
      chatgpt_user_id: 'user-fixture',
      email: 'fixture@example.com',
      plan_type: 'plus',
      chatgpt_account_is_fedramp: true,
    },
    tokens: {
      id_token: '',
      access_token: '',
      refresh_token: '',
    },
    created_at: 1,
    last_used: 1,
  };
}

function jwt(payload: Record<string, unknown>): string {
  const encode = (value: Record<string, unknown>) =>
    Buffer.from(JSON.stringify(value)).toString('base64url');
  return `${encode({ alg: 'none', typ: 'JWT' })}.${encode(payload)}.`;
}

test('Cockpit Tools export preserves portable Agent Identity credentials', () => {
  const raw = JSON.stringify([agentIdentityAccount()]);
  const exported = JSON.parse(
    transformCodexExportJson(raw, 'cockpit_tools'),
  ) as Array<Record<string, unknown>>;
  const account = exported[0];
  const identity = account.agent_identity as Record<string, unknown>;

  assert.equal(account.auth_mode, 'agentIdentity');
  assert.equal(account.type, 'codex');
  assert.equal(account.access_token, undefined);
  assert.equal(identity.agent_runtime_id, 'runtime-fixture');
  assert.equal(identity.agent_private_key, 'private-key-fixture');
  assert.equal(identity.task_id, 'task-fixture');
  assert.equal(identity.account_id, 'account-fixture');
  assert.equal(identity.chatgpt_user_id, 'user-fixture');
  assert.equal(identity.chatgpt_account_is_fedramp, true);
  assert.equal(hasCodexExportAgentIdentity(raw), true);
});

test('sub2api export preserves Agent Identity credentials', () => {
  const exported = JSON.parse(
    transformCodexExportJson(JSON.stringify([agentIdentityAccount()]), 'sub2api'),
  ) as { accounts: Array<{ credentials: Record<string, unknown> }> };
  const credentials = exported.accounts[0].credentials;

  assert.equal(credentials.auth_mode, 'agentIdentity');
  assert.equal(credentials.agent_runtime_id, 'runtime-fixture');
  assert.equal(credentials.agent_private_key, 'private-key-fixture');
  assert.equal(credentials.task_id, 'task-fixture');
  assert.equal(credentials.chatgpt_account_id, 'account-fixture');
  assert.equal(credentials.chatgpt_user_id, 'user-fixture');
});

test('CPA export rejects Agent Identity instead of producing empty tokens', () => {
  assert.throws(
    () => transformCodexExportJson(JSON.stringify([agentIdentityAccount()]), 'cpa'),
    /CPA format does not support Codex Agent Identity accounts/,
  );
});

test('regular token accounts keep their existing Cockpit Tools export shape', () => {
  const account: CodexAccount = {
    id: 'codex-token-fixture',
    email: 'token@example.com',
    account_id: 'account-token',
    tokens: {
      id_token: 'id-token-fixture',
      access_token: 'access-token-fixture',
      refresh_token: 'refresh-token-fixture',
    },
    created_at: 1,
    last_used: 1,
  };
  const exported = JSON.parse(
    transformCodexExportJson(JSON.stringify([account]), 'cockpit_tools'),
  ) as Array<Record<string, unknown>>;

  assert.equal(exported[0].access_token, 'access-token-fixture');
  assert.equal(exported[0].refresh_token, 'refresh-token-fixture');
  assert.equal(exported[0].agent_identity, undefined);
});

test('Codex export writes the official ChatGPT auth.json shape', () => {
  const account: CodexAccount = {
    id: 'codex-official-oauth',
    email: 'official@example.com',
    account_id: 'account-official',
    oauth_exchange_api_key: 'sk-exchange-result',
    token_updated_at: 1_700_000_000,
    tokens: {
      id_token: 'id-token-official',
      access_token: 'access-token-official',
      refresh_token: 'refresh-token-official',
    },
    created_at: 1,
    last_used: 1,
  };

  const exported = JSON.parse(
    transformCodexExportJson(JSON.stringify([account]), 'codex'),
  ) as Record<string, unknown>;

  assert.deepEqual(exported, {
    auth_mode: 'chatgpt',
    OPENAI_API_KEY: 'sk-exchange-result',
    tokens: {
      id_token: 'id-token-official',
      access_token: 'access-token-official',
      refresh_token: 'refresh-token-official',
      account_id: 'account-official',
    },
    last_refresh: '2023-11-14T22:13:20.000Z',
  });
});

test('Codex export keeps the official null API key and personal token shapes', () => {
  const oauth: CodexAccount = {
    id: 'codex-official-null-key',
    email: 'null-key@example.com',
    tokens: {
      id_token: 'id-token',
      access_token: 'access-token',
      refresh_token: 'refresh-token',
    },
    created_at: 1,
    last_used: 1,
  };
  const oauthExport = JSON.parse(
    transformCodexExportJson(JSON.stringify([oauth]), 'codex'),
  ) as Record<string, unknown>;
  assert.equal(oauthExport.auth_mode, 'chatgpt');
  assert.equal(oauthExport.OPENAI_API_KEY, null);
  assert.equal(oauthExport.type, undefined);
  assert.equal(oauthExport.email, undefined);

  const personalToken: CodexAccount = {
    id: 'codex-official-pat',
    email: 'pat@example.com',
    tokens: {
      id_token: '',
      access_token: 'at-personal-token',
    },
    created_at: 1,
    last_used: 1,
  };
  const personalTokenExport = JSON.parse(
    transformCodexExportJson(JSON.stringify([personalToken]), 'codex'),
  ) as Record<string, unknown>;
  assert.deepEqual(personalTokenExport, {
    auth_mode: 'personalAccessToken',
    OPENAI_API_KEY: null,
    personal_access_token: 'at-personal-token',
  });
});

test('Codex export supports official API key and Agent Identity shapes', () => {
  const apiKey: CodexAccount = {
    id: 'codex-official-api-key',
    email: 'API Key',
    auth_mode: 'apikey',
    openai_api_key: 'sk-official',
    tokens: { id_token: '', access_token: '' },
    created_at: 1,
    last_used: 1,
  };
  assert.deepEqual(
    JSON.parse(transformCodexExportJson(JSON.stringify([apiKey]), 'codex')),
    {
      auth_mode: 'apikey',
      OPENAI_API_KEY: 'sk-official',
    },
  );

  const agentIdentityExport = JSON.parse(
    transformCodexExportJson(
      JSON.stringify([agentIdentityAccount()]),
      'codex',
    ),
  ) as Record<string, unknown>;
  assert.equal(agentIdentityExport.auth_mode, 'agentIdentity');
  assert.equal(agentIdentityExport.type, undefined);
  assert.equal(agentIdentityExport.OPENAI_API_KEY, undefined);
  assert.equal(
    (agentIdentityExport.agent_identity as Record<string, unknown>).agent_runtime_id,
    'runtime-fixture',
  );
});

test('Codex export splits multiple accounts into one auth document per account', () => {
  const first: CodexAccount = {
    id: 'codex-first',
    email: 'first@example.com',
    tokens: {
      id_token: 'id-first',
      access_token: 'access-first',
      refresh_token: 'refresh-first',
    },
    created_at: 1,
    last_used: 1,
  };
  const second: CodexAccount = {
    ...first,
    id: 'codex-second',
    email: 'second@example.com',
    tokens: {
      id_token: 'id-second',
      access_token: 'access-second',
      refresh_token: 'refresh-second',
    },
  };

  const content = buildCodexExportContent(
    JSON.stringify([first, second]),
    'codex',
    'codex_accounts',
  );
  assert.equal(content.type, 'multiple');
  if (content.type !== 'multiple') return;
  assert.equal(content.documents.length, 2);
  assert.equal(content.documents[0].label, 'first@example.com');
  assert.equal(
    JSON.parse(content.documents[1].jsonContent).auth_mode,
    'chatgpt',
  );
  assert.match(content.documents[1].fileNameBase, /second@example\.com/);
});

test('sub2api OAuth export preserves official expiry and login-provider fields', () => {
  const accessToken = jwt({
    exp: 1_800_000_000,
    'https://api.openai.com/auth': {
      chatgpt_account_id: 'account-token',
      chatgpt_user_id: 'user-token',
      poid: 'org-token',
      chatgpt_plan_type: 'plus',
      chatgpt_subscription_active_until: '2090-01-01T00:00:00Z',
    },
  });
  const idToken = jwt({
    auth_provider: 'google',
  });
  const account: CodexAccount = {
    id: 'codex-token-fixture',
    email: 'token@example.com',
    account_id: 'account-token',
    plan_type: 'plus',
    subscription_active_until: '2027-01-02T03:04:05+00:00',
    codex_fingerprint_mode: 'full',
    tokens: {
      id_token: idToken,
      access_token: accessToken,
      refresh_token: 'refresh-token-fixture',
    },
    created_at: 1,
    last_used: 1,
  };

  const exported = JSON.parse(
    transformCodexExportJson(JSON.stringify([account]), 'sub2api'),
  ) as {
    accounts: Array<{
      type: string;
      credentials: Record<string, unknown>;
      extra?: Record<string, unknown>;
      expires_at?: number;
      concurrency: number;
      priority: number;
    }>;
  };
  const item = exported.accounts[0];

  assert.equal(item.type, 'oauth');
  assert.equal(item.credentials.client_id, 'app_EMoamEEZ73f0CkXaXp7hrann');
  assert.equal(item.credentials.expires_at, '2027-01-15T08:00:00.000Z');
  assert.equal(item.credentials.subscription_expires_at, '2027-01-02T03:04:05.000Z');
  assert.equal(item.credentials.chatgpt_user_id, 'user-token');
  assert.equal(item.credentials.organization_id, 'org-token');
  assert.equal(item.extra?.auth_provider, 'google');
  assert.equal(item.extra?.codex_fingerprint_mode, 'full');
  assert.equal(item.expires_at, undefined);
  assert.equal(item.concurrency, 3);
  assert.equal(item.priority, 50);
});

test('sub2api access-token-only export auto-pauses at token expiry', () => {
  const account: CodexAccount = {
    id: 'codex-at-only',
    email: 'at-only@example.com',
    tokens: {
      id_token: '',
      access_token: jwt({ exp: 1_800_000_000 }),
    },
    created_at: 1,
    last_used: 1,
  };

  const exported = JSON.parse(
    transformCodexExportJson(JSON.stringify([account]), 'sub2api'),
  ) as {
    accounts: Array<{
      expires_at?: number;
      auto_pause_on_expired?: boolean;
    }>;
  };

  assert.equal(exported.accounts[0].expires_at, 1_800_000_000);
  assert.equal(exported.accounts[0].auto_pause_on_expired, true);
});

test('sub2api API Key export uses native apikey credentials', () => {
  const account: CodexAccount = {
    id: 'codex-api-key',
    email: 'API Key',
    auth_mode: 'apikey',
    openai_api_key: 'sk-test',
    api_base_url: 'https://relay.example.com/v1',
    tokens: {
      id_token: '',
      access_token: '',
    },
    created_at: 1,
    last_used: 1,
  };

  const exported = JSON.parse(
    transformCodexExportJson(JSON.stringify([account]), 'sub2api'),
  ) as {
    accounts: Array<{ type: string; credentials: Record<string, unknown> }>;
  };

  assert.equal(exported.accounts[0].type, 'apikey');
  assert.deepEqual(exported.accounts[0].credentials, {
    base_url: 'https://relay.example.com/v1',
    api_key: 'sk-test',
  });
});
