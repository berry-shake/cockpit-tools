import { useCallback, useEffect, useRef, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { ArrowRightLeft, Check, Eye, EyeOff, FolderOpen, LayoutGrid, List, LogIn, Plus, RefreshCw, RotateCcw, Search, Trash2, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { OmpIcon } from '../components/icons/OmpIcon';
import { SingleSelectDropdown } from '../components/SingleSelectDropdown';
import { useGlobalModal } from '../hooks/useGlobalModal';
import { actOnOmpAccount, getOmpState, loginOmp, type OmpAccount, type OmpAccountAction, type OmpState } from '../services/ompService';
import './OmpPage.css';

export function OmpPage() {
  const { t } = useTranslation();
  const { showModal } = useGlobalModal();
  const [state, setState] = useState<OmpState | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [search, setSearch] = useState('');
  const [provider, setProvider] = useState('');
  const [trash, setTrash] = useState(false);
  const [layout, setLayout] = useState<'cards' | 'list'>('list');
  const [hideEmail, setHideEmail] = useState(false);
  const [adding, setAdding] = useState(false);
  const [executable, setExecutable] = useState('');
  const actionInFlight = useRef(false);

  const refresh = useCallback(async () => { setState(await getOmpState()); }, []);
  useEffect(() => {
    let cancelled = false;
    getOmpState().then((next) => {
      if (!cancelled) { setState(next); setExecutable(next.executable); }
    }).catch((e: unknown) => { if (!cancelled) setError(String(e)); })
      .finally(() => { if (!cancelled) setBusy(false); });
    return () => { cancelled = true; };
  }, []);

  async function run(action: () => Promise<void>) {
    if (actionInFlight.current) return;
    actionInFlight.current = true;
    setBusy(true); setError(''); setNotice('');
    try { await action(); } catch (e) { setError(String(e)); } finally {
      actionInFlight.current = false; setBusy(false);
    }
  }

  const accountTitle = (account: OmpAccount) => {
    const title = account.email || account.accountId || `${account.provider} #${account.id}`;
    return hideEmail ? `${title.slice(0, 2)}***` : title;
  };
  const statusText = (account: OmpAccount) => ({
    current: t('omp.account.providerSelected', '本供应商已选'),
    pool: t('omp.account.pool', 'OMP 自动选择'),
    standby: t('omp.account.standby', '待切换'),
    invalid: t('omp.account.invalid', '已停用 · 需重新登录'),
    trash: t('omp.account.trash', '回收站'),
  })[account.status];

  function confirmAction(account: OmpAccount, action: OmpAccountAction) {
    const title = action === 'switch' ? t('omp.account.switchTitle', '切换 OMP 账号')
      : action === 'trash' ? t('omp.account.trashTitle', '移入回收站') : t('omp.account.restoreTitle', '恢复账号');
    showModal({ title, description: action === 'switch'
      ? t('omp.account.switchDescription', '将 {{account}} 设为 {{provider}} 的唯一启用账号。同一供应商的其他账号会保留为待切换，不影响其他供应商。建议重启已有 OMP 会话；环境变量中的 API Key 和其他 profile 不受影响。', { account: accountTitle(account), provider: account.provider })
      : action === 'trash' ? t('omp.account.trashDescription', '停止在 OMP 中使用此账号，凭据保留在私密恢复文件中，可从回收站恢复。不会自动切换到其他账号。')
        : t('omp.account.restoreDescription', '恢复到账号列表，不会自动启用。恢复后点击“切换”即可使用。'),
      actions: [
        { label: t('common.cancel', '取消'), variant: 'secondary' },
        { label: title, variant: action === 'trash' ? 'danger' : 'primary', onClick: () => run(async () => {
          await actOnOmpAccount(account.id, account.identity, action);
          try { await refresh(); } catch { setError(t('omp.account.reloadAfterWrite', '账号操作已完成，但列表刷新失败，请点击刷新确认当前状态。')); }
          setNotice(action === 'switch'
            ? t('omp.account.switched', '已切换 OMP 原生账号。后续直接运行 omp 即可，Cockpit 无需常驻。建议重启已有会话。')
            : action === 'trash' ? t('omp.account.trashed', '已移入回收站，凭据仍可恢复。')
              : t('omp.account.restored', '账号已恢复，请检查账号状态后切换；原已失效的账号仍需重新登录。'));
        }) },
      ],
    });
  }

  const accounts = state?.accounts ?? [];
  const providers = [...new Set(accounts.map((a) => a.provider))].sort();
  const visible = accounts.filter((a) => (a.status === 'trash') === trash)
    .filter((a) => !provider || a.provider === provider)
    .filter((a) => `${a.email ?? ''} ${a.accountId ?? ''} ${a.orgName ?? ''} ${a.provider}`.toLowerCase().includes(search.trim().toLowerCase()));
  const groups = providers.map((name) => ({ name, accounts: visible.filter((a) => a.provider === name) }))
    .filter((group) => group.accounts.length > 0);
  const blocked = busy || Boolean(state?.warning) || !state;

  return <div className="omp-page">
    <header className="omp-header">
      <div><h1><OmpIcon size={28} /> OMP <span>{t('omp.account.management', '账号管理')}</span></h1>
        <p>{t('omp.account.subtitle', '独立账号 · 原生直连 · 一键切换')}</p></div>
      <div className="omp-header-actions">
        <button className="btn btn-secondary" disabled={busy} onClick={() => void run(refresh)}><RefreshCw size={16} className={busy ? 'spin' : ''} />{t('common.refresh', '刷新')}</button>
        <button className="btn btn-primary" disabled={busy} aria-expanded={adding} onClick={() => setAdding(!adding)}><Plus size={16} />{t('common.addAccount', '添加账号')}</button>
      </div>
    </header>
    <div className="omp-info">{t('omp.account.providerExplanation', '账号按供应商独立切换，不存在跨供应商的全局“当前账号”。同一供应商启用多个账号时，由 OMP 自动选择；登录与刷新由 OMP 完成，不经过 Cockpit API 服务。')}</div>
    {error && <div className="omp-error" role="alert">{error}</div>}
    {state?.warning && <div className="omp-error" role="alert">{state.warning}</div>}
    {notice && <div className="omp-notice" role="status">{notice}</div>}

    {adding && <section className="omp-panel omp-add-panel" aria-labelledby="omp-add-title">
      <div className="omp-panel-heading"><h2 id="omp-add-title">{t('omp.account.addTitle', '添加 OMP 账号')}</h2><button className="btn btn-secondary" aria-label={t('common.close', '关闭')} onClick={() => setAdding(false)}><X size={16} /></button></div>
      <p>{t('omp.account.loginHelp', '打开 OMP 原生登录流程，在终端选择供应商并完成浏览器授权。完成后点击“刷新”，新账号会出现在此列表，无需新建 profile 或选择工作目录。')}</p>
      <label htmlFor="omp-executable">{t('omp.executable', 'OMP 可执行文件')}</label>
      <div className="omp-path-row"><input id="omp-executable" value={executable} disabled={busy} placeholder="/Users/you/.bun/bin/omp" onChange={(e) => setExecutable(e.target.value)} />
        <button className="btn btn-secondary" disabled={busy} aria-label={t('omp.chooseExecutable', '选择 OMP 可执行文件')} onClick={() => void run(async () => { const path = await open({ multiple: false }); if (typeof path === 'string') setExecutable(path); })}><FolderOpen size={16} /></button>
        <button className="btn btn-primary" disabled={blocked || !state?.loginSupported || !executable.trim()} onClick={() => void run(async () => {
          await loginOmp(executable.trim());
          setNotice(t('omp.account.loginSent', '已向 Terminal 发送原生登录命令；尚未确认登录成功。请在终端完成授权，然后刷新账号列表。'));
        })}><LogIn size={16} />{t('omp.account.nativeLogin', '开始原生登录')}</button>
      </div>
      {!state?.loginSupported && <p>{t('omp.macosOnly', '原生终端登录暂仅支持 macOS。')}</p>}
    </section>}

    <div className="omp-toolbar">
      <label className="omp-search"><Search size={16} /><input aria-label={t('omp.account.search', '搜索账号')} placeholder={t('omp.account.searchPlaceholder', '搜索邮箱、账号或供应商…')} value={search} onChange={(e) => setSearch(e.target.value)} /></label>
      <SingleSelectDropdown className="omp-provider-filter" menuClassName="omp-provider-menu"
        ariaLabel={t('omp.account.providerFilter', '供应商筛选')} value={provider} onChange={setProvider}
        options={[{ value: '', label: t('omp.account.allProviders', '全部供应商') }, ...providers.map((p) => ({ value: p, label: p }))]} />
      <button className="btn btn-secondary" aria-label={layout === 'cards' ? t('omp.account.listView', '列表视图') : t('omp.account.cardView', '卡片视图')} onClick={() => setLayout(layout === 'cards' ? 'list' : 'cards')}>{layout === 'cards' ? <List size={17} /> : <LayoutGrid size={17} />}</button>
      <button className="btn btn-secondary" aria-pressed={hideEmail} onClick={() => setHideEmail(!hideEmail)}>{hideEmail ? <Eye size={16} /> : <EyeOff size={16} />}{hideEmail ? t('omp.account.showIdentity', '显示账号') : t('omp.account.hideIdentity', '隐藏账号')}</button>
    </div>
    <div className="omp-tabs"><button className={!trash ? 'selected' : ''} aria-pressed={!trash} onClick={() => setTrash(false)}>{t('omp.account.all', '全部账号')} <span>{accounts.filter((a) => a.status !== 'trash').length}</span></button>
      <button className={trash ? 'selected' : ''} aria-pressed={trash} onClick={() => setTrash(true)}><Trash2 size={15} />{t('omp.account.recycleBin', '回收站')} <span>{accounts.filter((a) => a.status === 'trash').length}</span></button></div>
    <div className={`omp-account-groups ${layout}`} aria-busy={busy}>
      {groups.map((group) => <section className="omp-provider-group" key={group.name} aria-label={group.name}>
        <div className="omp-group-heading"><h2>{group.name}</h2><span>{group.accounts.length}</span></div>
        <div className="omp-account-grid">
      {group.accounts.map((account) => <article className={`omp-account-card ${account.status}`} key={account.id}>
        <div className="omp-account-main"><h3 title={hideEmail ? undefined : accountTitle(account)}>{accountTitle(account)}</h3>
          <div className="omp-account-meta"><span>{account.credentialType === 'oauth' ? 'OAuth' : 'API Key'}</span>{account.orgName && <span>{hideEmail ? '***' : account.orgName}</span>}</div>
          {!account.inNativeStore && <p className="omp-muted">{t('omp.account.recovery', '凭据保存在本地恢复文件中；切换时恢复到原生账号库。')}</p>}
        </div>
        <div className="omp-account-actions">
          <span className={`omp-badge ${account.status}`}>{account.status === 'current' && <Check size={13} />}{statusText(account)}</span>
          {account.status === 'trash'
          ? <button className="btn btn-secondary" disabled={blocked} onClick={() => confirmAction(account, 'restore')}><RotateCcw size={15} />{t('omp.account.restore', '恢复')}</button>
          : <>
            <button className="btn btn-secondary omp-switch-button" disabled={blocked || account.status === 'current' || account.status === 'invalid'} onClick={() => confirmAction(account, 'switch')}><ArrowRightLeft size={15} />{t('omp.account.switch', '切换')}</button>
            <button className="btn btn-secondary omp-trash-button" disabled={blocked} title={t('omp.account.trashTitle', '移入回收站')} aria-label={t('omp.account.trashTitle', '移入回收站')} onClick={() => confirmAction(account, 'trash')}><Trash2 size={15} /></button></>}
        </div>
      </article>)}</div></section>)}
    </div>
    {!visible.length && <div className="omp-empty">{busy ? t('common.loading', '加载中…') : search || provider ? t('omp.account.noMatches', '没有匹配的账号') : trash ? t('omp.account.emptyTrash', '回收站为空') : t('omp.account.noAccounts', '还没有 OMP 账号，点击“添加账号”开始登录。')}</div>}
    <footer className="omp-footnote"><p>{t('omp.account.storeNote', '仅管理默认 ~/.omp 账号库；不修改其他 profile、环境变量或供应商配置。已启用不代表已通过在线验证。')}</p><code>{state?.databasePath}</code></footer>
  </div>;
}
