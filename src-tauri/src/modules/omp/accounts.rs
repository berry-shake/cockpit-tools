//! Cockpit selects one native credential per provider. Inactive credentials
//! are retained, not copied to another running client or refreshed here.
//! OMP may purge disabled API-key rows on login, so a private recovery archive
//! must be durably written BEFORE changing the native database.
use super::*;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const STANDBY: &str = "cockpit:standby";
const TRASH: &str = "cockpit:trash";
static ACCOUNT_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Clone, Deserialize, Serialize)]
struct StoredAccount {
    id: i64,
    provider: String,
    credential_type: String,
    data: String,
    disabled_cause: Option<String>,
    identity_key: Option<String>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Default, Deserialize, Serialize)]
struct Archive {
    // Deliberately not an OMP auth export: only this app reads the recovery file.
    version: u32,
    accounts: BTreeMap<i64, StoredAccount>,
    #[serde(default)]
    prior_disabled: BTreeMap<i64, String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    id: i64,
    identity: String,
    provider: String,
    credential_type: String,
    email: Option<String>,
    account_id: Option<String>,
    org_name: Option<String>,
    // current = the only enabled native account for this provider, not a claim
    // about an in-flight request or an environment-variable API-key override.
    status: String,
    in_native_store: bool,
}

fn identity(row: &StoredAccount) -> String {
    let json: serde_json::Value = serde_json::from_str(&row.data).unwrap_or_default();
    let value = serde_json::json!([
        row.id,
        row.provider,
        row.credential_type,
        row.identity_key,
        json.get("email"),
        json.get("accountId"),
        json.get("orgId")
    ]);
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

fn same_account(a: &StoredAccount, b: &StoredAccount) -> bool {
    if a.provider != b.provider || a.credential_type != b.credential_type {
        return false;
    }
    if a.credential_type == "oauth" {
        return a
            .identity_key
            .as_ref()
            .is_some_and(|key| !key.is_empty() && b.identity_key.as_ref() == Some(key));
    }
    let a: serde_json::Value = serde_json::from_str(&a.data).unwrap_or_default();
    let b: serde_json::Value = serde_json::from_str(&b.data).unwrap_or_default();
    a.get("key")
        .and_then(|v| v.as_str())
        .is_some_and(|key| !key.is_empty() && b.get("key").and_then(|v| v.as_str()) == Some(key))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmpAccountState {
    pub executable: String,
    accounts: Vec<Account>,
    pub database_path: String,
    warning: Option<String>,
    login_supported: bool,
}

fn archive_path() -> Result<PathBuf, String> {
    Ok(super::super::account::get_data_dir()?.join("omp_accounts_recovery.json"))
}

fn ordinary_file(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() || !m.is_file() => {
            Err("OMP 账号文件不是普通文件；未修改".into())
        }
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("无法检查 OMP 账号文件".into()),
    }
}

fn read_archive(path: &Path) -> Result<Archive, String> {
    if !ordinary_file(path)? {
        return Ok(Archive {
            version: 1,
            ..Archive::default()
        });
    }
    let raw = fs::read_to_string(path).map_err(|_| "无法读取 OMP 账号恢复文件")?;
    let archive: Archive =
        serde_json::from_str(&raw).map_err(|_| "OMP 账号恢复文件损坏；未覆盖")?;
    if archive.version != 1 {
        return Err("OMP 账号恢复文件版本不兼容；未覆盖".into());
    }
    for (id, row) in &archive.accounts {
        if *id != row.id || row.id <= 0 || row.provider.starts_with("mcp_oauth:") {
            return Err("OMP 账号恢复文件格式不兼容；未覆盖".into());
        }
    }
    Ok(archive)
}

fn save_archive(path: &Path, archive: &Archive) -> Result<(), String> {
    ordinary_file(path)?;
    let raw = serde_json::to_string(archive).map_err(|_| "无法保存 OMP 账号备份")?;
    // This helper creates the temporary file as 0600 before writing secrets.
    super::super::atomic_write::write_secret_string_atomic(path, &raw)
        .map_err(|_| "无法安全保存 OMP 账号备份；原生账号未修改")?;
    #[cfg(unix)]
    fs::File::open(path.parent().ok_or("无法定位恢复文件目录")?)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| "无法同步 OMP 账号备份目录；原生账号未修改")?;
    Ok(())
}

fn archive_write_lock(path: &Path) -> Result<fs::File, String> {
    let lock_path = path.with_extension("json.lock");
    ordinary_file(&lock_path)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|_| "无法打开 OMP 账号恢复文件锁")?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|_| "其他 Cockpit 进程正在操作 OMP 账号，请稍后重试")?;
    Ok(lock)
}

fn open_database(root: &Path, write: bool) -> Result<Option<Connection>, String> {
    check_profile_path(root, "default")?;
    if broker_configured(&root.join("agent"))? {
        return Err("默认 OMP 配置使用认证 broker，本页仅管理原生本地账号；未修改配置".into());
    }
    let path = root.join("agent/agent.db");
    if !ordinary_file(&path)? {
        return Ok(None);
    }
    for suffix in ["agent.db-wal", "agent.db-shm", "agent.db-journal"] {
        ordinary_file(&root.join("agent").join(suffix))?;
    }
    let flags = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let db = Connection::open_with_flags(&path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| "无法打开 OMP 原生账号数据库")?;
    db.busy_timeout(std::time::Duration::from_millis(1500))
        .map_err(|_| "OMP 数据库繁忙")?;
    // Never initialize/migrate OMP-owned tables. Version 8 is the inspected
    // native contract, including refresh fencing and external-change tracking.
    let version: Option<i64> = db
        .query_row(
            "SELECT version FROM auth_schema_version WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| "OMP 认证数据库版本不兼容，请先用 OMP 原生登录初始化")?;
    if version != Some(8) {
        return Err("OMP 认证数据库版本不兼容；为保护凭据，未修改".into());
    }
    Ok(Some(db))
}

fn records(db: &Connection) -> Result<BTreeMap<i64, StoredAccount>, String> {
    let mut query = db.prepare("SELECT id, provider, credential_type, data, disabled_cause, identity_key, created_at, updated_at FROM auth_credentials WHERE provider NOT LIKE 'mcp_oauth:%' ORDER BY id")
        .map_err(|_| "OMP 认证表结构不兼容")?;
    let rows = query
        .query_map([], |r| {
            Ok(StoredAccount {
                id: r.get(0)?,
                provider: r.get(1)?,
                credential_type: r.get(2)?,
                data: r.get(3)?,
                disabled_cause: r.get(4)?,
                identity_key: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
            })
        })
        .map_err(|_| "无法读取 OMP 账号")?;
    rows.map(|r| r.map(|r| (r.id, r)))
        .collect::<Result<_, _>>()
        .map_err(|_| "OMP 账号记录不兼容".into())
}

fn summaries(native: &BTreeMap<i64, StoredAccount>, archive: Archive) -> Vec<Account> {
    let mut all = archive.accounts;
    // A fresh native login can replace a disabled row using a NEW row ID.
    // Do not offer its stale archived refresh token as a second account.
    all.retain(|id, old| {
        native.contains_key(id) || !native.values().any(|row| same_account(row, old))
    });
    // Native data always wins: never restore a stale token from a backup over
    // a live credential that OMP has refreshed since the last UI operation.
    all.extend(native.clone());
    all.values()
        .map(|r| {
            let value: serde_json::Value = serde_json::from_str(&r.data).unwrap_or_default();
            let field = |key| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
            let active_count = native
                .values()
                .filter(|other| other.provider == r.provider && other.disabled_cause.is_none())
                .count();
            let status = match r.disabled_cause.as_deref() {
                Some(TRASH) => "trash",
                Some(STANDBY) => "standby",
                Some(_) => "invalid",
                None if !native.contains_key(&r.id) => "standby",
                None if active_count == 1 => "current",
                None => "pool",
            };
            Account {
                id: r.id,
                identity: identity(r),
                provider: r.provider.clone(),
                credential_type: r.credential_type.clone(),
                email: field("email"),
                account_id: field("accountId"),
                org_name: field("orgName"),
                status: status.into(),
                in_native_store: native.contains_key(&r.id),
            }
        })
        .collect()
}

pub fn account_state() -> Result<OmpAccountState, String> {
    let _guard = ACCOUNT_LOCK.lock().map_err(|_| "OMP 账号锁不可用")?;
    let root = root_dir()?;
    let archive = read_archive(&archive_path()?)?;
    let read = open_database(&root, false).and_then(|db| db.map(|db| records(&db)).transpose());
    let (native, warning) = match read {
        Ok(rows) => (rows.unwrap_or_default(), None),
        Err(e) => (BTreeMap::new(), Some(e)),
    };
    Ok(OmpAccountState {
        executable: load_settings()?.executable,
        accounts: summaries(&native, archive),
        database_path: root.join("agent/agent.db").to_string_lossy().into(),
        warning,
        login_supported: cfg!(target_os = "macos"),
    })
}

fn mutate_at(
    root: &Path,
    archive_path: &Path,
    id: i64,
    expected_identity: &str,
    action: &str,
) -> Result<(), String> {
    if !["switch", "trash", "restore"].contains(&action) || id <= 0 {
        return Err("无效的 OMP 账号操作".into());
    }
    // Retain this lock through the final archive save, including after SQLite
    // commit. Token refreshes in OMP do not need or use this Cockpit-only lock.
    let _file_guard = archive_write_lock(archive_path)?;
    let mut db = open_database(root, true)?.ok_or("请先用 OMP 原生登录创建本地账号数据库")?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "OMP 正在写入账号，请稍后重试")?;
    // Load recovery state under both locks; token snapshots come from this tx.
    let mut archive = read_archive(archive_path)?;
    let native = records(&tx)?;
    let mut target = native
        .get(&id)
        .or_else(|| archive.accounts.get(&id))
        .cloned()
        .ok_or("账号已不存在，请刷新")?;
    if !native.contains_key(&id) && native.values().any(|row| same_account(row, &target)) {
        return Err("此账号已在 OMP 中重新登录，请刷新后使用最新账号；未恢复旧凭据".into());
    }
    if identity(&target) != expected_identity {
        return Err("账号身份已变化，请刷新后重新操作".into());
    }
    let cause = target.disabled_cause.as_deref();
    if action == "switch" && cause.is_some_and(|c| c != STANDBY) {
        return Err("此账号已失效或在回收站中，请先重新登录或恢复".into());
    }
    if action == "restore" && cause != Some(TRASH) {
        return Err("此账号不在回收站中，请刷新".into());
    }
    if action == "trash" && cause == Some(TRASH) {
        return Err("此账号已在回收站中，请刷新".into());
    }
    let valid: serde_json::Value =
        serde_json::from_str(&target.data).map_err(|_| "账号凭据损坏，请重新登录")?;
    if !valid.is_object() || !matches!(target.credential_type.as_str(), "oauth" | "api_key") {
        return Err("账号凭据格式不兼容，请重新登录".into());
    }
    // A refresh may already have rotated the remote token but not committed it.
    // Fail closed until its native lease expires/releases; never strand it.
    let refreshing: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM auth_credential_refresh_leases l JOIN auth_credentials c ON c.id=l.credential_id WHERE c.provider=?1 AND l.expires_at_ms > ?2)",
        params![target.provider, chrono::Utc::now().timestamp_millis()], |r| r.get(0))
        .map_err(|_| "无法确认 OMP 刷新状态；未修改账号")?;
    if refreshing {
        return Err("OMP 正在刷新该供应商的凭据，请稍后重试切换；未修改账号".into());
    }
    // Require OMP's own update trigger so live processes notice the change.
    let tracked: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='auth_change_revision_auth_credentials_update')", [], |r| r.get(0))
        .map_err(|_| "无法检查 OMP 账号变更通知")?;
    if !tracked {
        return Err("OMP 数据库缺少原生账号变更通知；请先启动新版 OMP".into());
    }
    for row in native.values().filter(|r| r.provider == target.provider) {
        archive.accounts.insert(row.id, row.clone());
    }
    // Snapshot committed state before any write. A rejected native transaction
    // must not alter the visible recovery/trash state of archived-only rows.
    save_archive(archive_path, &archive)?;
    if action == "trash" {
        if let Some(cause) = target.disabled_cause.as_deref().filter(|c| *c != STANDBY) {
            archive.prior_disabled.insert(id, cause.into());
        } else {
            archive.prior_disabled.remove(&id);
        }
    }
    target.disabled_cause = match action {
        "switch" => None,
        "trash" => Some(TRASH.into()),
        _ => Some(
            archive
                .prior_disabled
                .remove(&id)
                .unwrap_or_else(|| STANDBY.into()),
        ),
    };
    target.updated_at = chrono::Utc::now().timestamp();
    archive.accounts.insert(id, target.clone());
    if action == "switch" {
        for row in archive
            .accounts
            .values_mut()
            .filter(|r| r.provider == target.provider && r.id != id && r.disabled_cause.is_none())
        {
            row.disabled_cause = Some(STANDBY.into());
        }
    }
    if !native.contains_key(&id) {
        // Explicit ID is safe only while it remains unused, including MCP rows.
        tx.execute("INSERT INTO auth_credentials (id,provider,credential_type,data,disabled_cause,identity_key,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![target.id, target.provider, target.credential_type, target.data, target.disabled_cause, target.identity_key, target.created_at, target.updated_at])
            .map_err(|_| "原生账号标识已变化，恢复已取消；备份仍保留")?;
    }
    if action == "switch" {
        tx.execute("UPDATE auth_credentials SET disabled_cause=?1, updated_at=?2 WHERE provider=?3 AND id<>?4 AND disabled_cause IS NULL",
            params![STANDBY, target.updated_at, target.provider, id]).map_err(|_| "切换账号失败；原生事务已回滚")?;
    }
    tx.execute(
        "UPDATE auth_credentials SET disabled_cause=?1, updated_at=?2 WHERE id=?3 AND provider=?4",
        params![
            target.disabled_cause,
            target.updated_at,
            id,
            target.provider
        ],
    )
    .map_err(|_| "更新账号失败；原生事务已回滚")?;
    tx.commit()
        .map_err(|_| "提交账号切换失败；请刷新确认原生状态")?;
    save_archive(archive_path, &archive)
        .map_err(|_| "原生账号操作已完成，但备份状态更新失败；凭据备份仍保留，请刷新确认".into())
}

pub fn account_action(id: i64, expected_identity: String, action: String) -> Result<(), String> {
    let _guard = ACCOUNT_LOCK.lock().map_err(|_| "OMP 账号锁不可用")?;
    mutate_at(
        &root_dir()?,
        &archive_path()?,
        id,
        &expected_identity,
        &action,
    )
}

#[cfg(test)]
mod tests;
