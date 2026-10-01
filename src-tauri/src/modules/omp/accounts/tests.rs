use super::*;

struct Fixture {
    root: PathBuf,
    archive: PathBuf,
    db: Connection,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("cockpit-omp-accounts-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("agent")).unwrap();
        let db = Connection::open(root.join("agent/agent.db")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE auth_schema_version (id INTEGER PRIMARY KEY, version INTEGER); INSERT INTO auth_schema_version VALUES (1,8);
            CREATE TABLE auth_credentials (id INTEGER PRIMARY KEY AUTOINCREMENT, provider TEXT NOT NULL, credential_type TEXT NOT NULL, data TEXT NOT NULL, disabled_cause TEXT, identity_key TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
            CREATE TABLE auth_credential_refresh_leases (credential_id INTEGER PRIMARY KEY, owner TEXT, expires_at_ms INTEGER, updated_at INTEGER);
            CREATE TABLE auth_change_revision (id INTEGER PRIMARY KEY, revision INTEGER); INSERT INTO auth_change_revision VALUES (1,0);
            CREATE TRIGGER auth_change_revision_auth_credentials_update AFTER UPDATE ON auth_credentials BEGIN UPDATE auth_change_revision SET revision=revision+1 WHERE id=1; END;
            CREATE TABLE unrelated (value TEXT); INSERT INTO unrelated VALUES ('preserve');").unwrap();
        for (id, provider, kind, cause) in [
            (1, "openai-codex", "oauth", None),
            (2, "openai-codex", "oauth", None),
            (3, "anthropic", "oauth", None),
            (4, "openai-codex", "oauth", Some("revoked")),
            (5, "mcp_oauth:fixture", "oauth", None),
            (6, "fixture-key", "api_key", None),
            (7, "fixture-key", "api_key", None),
        ] {
            let data = serde_json::json!({"email":format!("test{id}@example.invalid"),"access":format!("secret-access-{id}"),"refresh":format!("secret-refresh-{id}"),"key":format!("secret-key-{id}"),"custom":{"preserve":true}}).to_string();
            db.execute(
                "INSERT INTO auth_credentials VALUES (?1,?2,?3,?4,?5,?6,1,1)",
                params![id, provider, kind, data, cause, format!("account:{id}")],
            )
            .unwrap();
        }
        Self {
            archive: root.join("recovery.json"),
            root,
            db,
        }
    }
    fn act(&self, id: i64, action: &str) -> Result<(), String> {
        let native = records(&self.db).unwrap();
        let archive = read_archive(&self.archive)?;
        let row = native
            .get(&id)
            .or_else(|| archive.accounts.get(&id))
            .unwrap();
        mutate_at(&self.root, &self.archive, id, &identity(row), action)
    }
    fn cause(&self, id: i64) -> Option<String> {
        self.db
            .query_row(
                "SELECT disabled_cause FROM auth_credentials WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .unwrap()
    }
    fn data(&self, id: i64) -> String {
        self.db
            .query_row("SELECT data FROM auth_credentials WHERE id=?", [id], |r| {
                r.get(0)
            })
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
#[test]
fn shared_data_alias_preserves_recovery_archive_locking_and_native_credentials() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let mut f = Fixture::new();
    let legacy = f.root.join(".antigravity_cockpit");
    let alias = f.root.join(".cockpit_tools");
    fs::create_dir(&legacy).unwrap();
    f.archive = legacy.join("omp_accounts_recovery.json");
    let original: Vec<_> = (1..=7).map(|id| f.data(id)).collect();
    f.act(2, "switch").unwrap();
    symlink(&legacy, &alias).unwrap();
    let aliased_archive = alias.join("omp_accounts_recovery.json");
    let selected = records(&f.db).unwrap()[&1].clone();
    let before = fs::read(&f.archive).unwrap();
    {
        // Old and new processes must contend on the same recovery-file lock.
        let _old_process_lock = archive_write_lock(&f.archive).unwrap();
        assert!(
            mutate_at(&f.root, &aliased_archive, 1, &identity(&selected), "switch")
                .unwrap_err()
                .contains("其他 Cockpit")
        );
        assert_eq!(fs::read(&f.archive).unwrap(), before);
        assert_eq!(f.cause(1).as_deref(), Some(STANDBY));
    }
    mutate_at(&f.root, &aliased_archive, 1, &identity(&selected), "switch").unwrap();
    assert_eq!(f.cause(1), None);
    assert_eq!(f.cause(2).as_deref(), Some(STANDBY));
    assert_eq!(f.cause(3), None);
    assert_eq!(original, (1..=7).map(|id| f.data(id)).collect::<Vec<_>>());
    assert_eq!(
        fs::canonicalize(&f.archive).unwrap(),
        fs::canonicalize(&aliased_archive).unwrap()
    );
    assert!(read_archive(&f.archive).unwrap().accounts[&1]
        .disabled_cause
        .is_none());
    assert_eq!(
        fs::metadata(&f.archive).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(f.root.join("agent/agent.db").is_file());
    assert!(!alias.join("agent").exists());
}

#[test]
fn switching_selects_exactly_one_provider_account_and_preserves_all_credential_bytes() {
    let f = Fixture::new();
    let data: Vec<_> = (1..=7).map(|id| f.data(id)).collect();
    f.act(2, "switch").unwrap();
    assert_eq!(f.cause(1).as_deref(), Some(STANDBY));
    assert_eq!(f.cause(2), None);
    assert_eq!(f.cause(3), None);
    assert_eq!(f.cause(4).as_deref(), Some("revoked"));
    assert_eq!(f.cause(5), None);
    assert_eq!(data, (1..=7).map(|id| f.data(id)).collect::<Vec<_>>());
    f.act(1, "switch").unwrap();
    assert_eq!(f.cause(1), None);
    assert_eq!(f.cause(2).as_deref(), Some(STANDBY));
    assert!(
        f.db.query_row("SELECT revision FROM auth_change_revision", [], |r| r
            .get::<_, i64>(0))
            .unwrap()
            > 0
    );
    assert_eq!(
        f.db.query_row("SELECT value FROM unrelated", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "preserve"
    );
}

#[test]
fn refresh_lease_prevents_switching_and_newest_native_token_wins_over_recovery() {
    let f = Fixture::new();
    f.act(2, "switch").unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    f.db.execute(
        "INSERT INTO auth_credential_refresh_leases VALUES (2,'fixture',?1,1)",
        [now + 60_000],
    )
    .unwrap();
    let archive_before = fs::read(&f.archive).unwrap();
    assert!(f.act(1, "switch").unwrap_err().contains("正在刷新"));
    assert_eq!(fs::read(&f.archive).unwrap(), archive_before);
    f.db.execute("UPDATE auth_credentials SET data=json_set(data,'$.refresh','secret-new-refresh') WHERE id=2",[]).unwrap();
    f.db.execute("DELETE FROM auth_credential_refresh_leases", [])
        .unwrap();
    f.act(1, "switch").unwrap();
    f.act(2, "switch").unwrap();
    assert!(f.data(2).contains("secret-new-refresh"));
    assert!(read_archive(&f.archive).unwrap().accounts[&2]
        .data
        .contains("secret-new-refresh"));
}

#[test]
fn native_api_key_tombstone_cleanup_cannot_lose_a_switchable_account() {
    let f = Fixture::new();
    let original = f.data(6);
    f.act(7, "switch").unwrap();
    // Native OMP removes disabled API-key rows when a replacement is logged in.
    f.db.execute("DELETE FROM auth_credentials WHERE id=6", [])
        .unwrap();
    let view = summaries(&records(&f.db).unwrap(), read_archive(&f.archive).unwrap());
    let archived = view.iter().find(|a| a.id == 6).unwrap();
    assert_eq!(archived.status, "standby");
    assert!(!archived.in_native_store);
    f.act(6, "switch").unwrap();
    assert_eq!(f.data(6), original);
    assert_eq!(f.cause(6), None);
    assert_eq!(f.cause(7).as_deref(), Some(STANDBY));
}

#[test]
fn a_new_native_login_supersedes_archived_tokens_even_when_its_row_id_changes() {
    let f = Fixture::new();
    f.act(2, "switch").unwrap();
    let old = read_archive(&f.archive).unwrap().accounts[&1].clone();
    f.db.execute("DELETE FROM auth_credentials WHERE id=1", [])
        .unwrap();
    f.db.execute(
        "INSERT INTO auth_credentials VALUES (20,'openai-codex','oauth',?1,NULL,'account:1',1,2)",
        [old.data.replace("secret-refresh-1", "secret-fresh-login")],
    )
    .unwrap();
    let view = summaries(&records(&f.db).unwrap(), read_archive(&f.archive).unwrap());
    assert!(!view.iter().any(|a| a.id == 1));
    assert!(view.iter().any(|a| a.id == 20));
    assert!(mutate_at(&f.root, &f.archive, 1, &identity(&old), "switch")
        .unwrap_err()
        .contains("重新登录"));
    assert!(f.data(20).contains("secret-fresh-login"));
}

#[test]
fn another_cockpit_archive_writer_blocks_mutation_without_losing_native_state() {
    let f = Fixture::new();
    let _lock = archive_write_lock(&f.archive).unwrap();
    assert!(f.act(1, "switch").unwrap_err().contains("其他 Cockpit"));
    assert_eq!(f.cause(2), None);
    assert!(!f.archive.exists());
}

#[test]
fn recycle_bin_is_recoverable_and_restore_does_not_enable_revoked_or_standby_accounts() {
    let f = Fixture::new();
    f.act(2, "switch").unwrap();
    f.act(2, "trash").unwrap();
    assert_eq!(f.cause(2).as_deref(), Some(TRASH));
    assert!(f.act(2, "switch").is_err());
    f.act(2, "restore").unwrap();
    assert_eq!(f.cause(2).as_deref(), Some(STANDBY));
    f.act(4, "trash").unwrap();
    f.act(4, "restore").unwrap();
    assert_eq!(f.cause(4).as_deref(), Some("revoked"));
    assert!(f.act(4, "switch").is_err());
}

#[test]
fn snapshot_failure_schema_change_or_missing_tracking_prevents_native_mutation() {
    let f = Fixture::new();
    fs::create_dir(&f.archive).unwrap();
    assert!(f.act(1, "switch").is_err());
    assert_eq!(f.cause(2), None);
    fs::remove_dir(&f.archive).unwrap();
    f.db.execute("UPDATE auth_schema_version SET version=9", [])
        .unwrap();
    assert!(f.act(1, "switch").is_err());
    assert!(!f.archive.exists());
    f.db.execute("UPDATE auth_schema_version SET version=8", [])
        .unwrap();
    f.db.execute(
        "DROP TRIGGER auth_change_revision_auth_credentials_update",
        [],
    )
    .unwrap();
    assert!(f.act(1, "switch").is_err());
    assert_eq!(f.cause(2), None);
}

#[test]
fn native_transaction_rollback_keeps_recovery_only_accounts_in_their_previous_state() {
    let f = Fixture::new();
    f.act(6, "trash").unwrap();
    f.db.execute("DELETE FROM auth_credentials WHERE id=6", [])
        .unwrap();
    f.db.execute_batch("CREATE TRIGGER fail_restore BEFORE INSERT ON auth_credentials BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
    assert!(f.act(6, "restore").is_err());
    assert_eq!(
        read_archive(&f.archive).unwrap().accounts[&6]
            .disabled_cause
            .as_deref(),
        Some(TRASH)
    );
}

#[test]
fn summaries_are_secret_free_and_never_claim_one_current_account_for_a_pool() {
    let f = Fixture::new();
    let native = records(&f.db).unwrap();
    let view = summaries(&native, read_archive(&f.archive).unwrap());
    assert_eq!(view.iter().find(|a| a.id == 1).unwrap().status, "pool");
    assert_eq!(view.iter().find(|a| a.id == 3).unwrap().status, "current");
    let json = serde_json::to_string(&view).unwrap();
    assert!(!json.contains("secret-"));
    assert!(!json.contains("mcp_oauth"));
    assert!(!f.archive.exists());
}

#[test]
fn identity_change_blocks_a_stale_ui_action_but_token_refresh_does_not() {
    let f = Fixture::new();
    let expected = identity(&records(&f.db).unwrap()[&1]);
    f.db.execute(
        "UPDATE auth_credentials SET identity_key='different-account' WHERE id=1",
        [],
    )
    .unwrap();
    assert!(mutate_at(&f.root, &f.archive, 1, &expected, "switch")
        .unwrap_err()
        .contains("身份已变化"));
    assert_eq!(f.cause(2), None);
    assert!(!f.archive.exists());
}

#[cfg(unix)]
#[test]
fn archives_are_private_and_symlinked_databases_and_archives_are_rejected() {
    use std::os::unix::{fs::symlink, fs::PermissionsExt};
    let f = Fixture::new();
    f.act(1, "switch").unwrap();
    assert_eq!(
        fs::metadata(&f.archive).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!f.root.join("recovery.json.bak").exists());
    fs::rename(&f.archive, f.root.join("original.json")).unwrap();
    symlink(f.root.join("original.json"), &f.archive).unwrap();
    assert!(mutate_at(&f.root, &f.archive, 1, "fixture", "switch").is_err());
    fs::rename(f.root.join("agent/agent.db"), f.root.join("original.db")).unwrap();
    symlink(f.root.join("original.db"), f.root.join("agent/agent.db")).unwrap();
    assert!(open_database(&f.root, true).is_err());
}
