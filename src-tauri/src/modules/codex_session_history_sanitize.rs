//! 会话历史清洗：把第三方（DeepSeek 等）产生的 reasoning 项清成官方可接受的形状。
//!
//! 背景：官方 Codex 后端要求 reasoning 项的 `content` 必须是空数组。第三方提供商在历史里
//! 留下带可见思考文本的 reasoning 项（`content` 非空），同一会话切到官方账号后整段请求会被拒：
//! `[ArrayParam] [input[i].content] [array_above_max_length]`，普通回合与自动压缩都无法继续。
//!
//! 这里在切号/启动（客户端已关闭）时，把客户端历史库 `thread_history_*.sqlite` 里
//! `thread_items` 的这类项清空 `content`，使旧会话在官方账号下
//! 也能继续使用。写入前用 `VACUUM INTO` 生成一致备份，操作幂等、可重复执行。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::Connection;
use serde_json::Value;

use crate::modules;

const HISTORY_DB_STEM: &str = "thread_history";
const HISTORY_DB_EXTENSION: &str = "sqlite";
const SQLITE_DIR_NAME: &str = "sqlite";
const BACKUP_DIR_NAME: &str = "cockpit-history-sanitize-backup";
const MAX_BACKUP_DIRS: usize = 3;
const BUSY_TIMEOUT_SECONDS: u64 = 5;
const ITEM_TABLES: [&str; 2] = ["thread_items", "thread_realtime_items"];

static HISTORY_SANITIZE_LOCK: Mutex<()> = Mutex::new(());

/// 会话历史清洗结果，用于日志与上层判断是否需要提示。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CodexSessionHistorySanitizeSummary {
    pub database_count: usize,
    pub updated_item_count: usize,
    pub changed_thread_count: usize,
}

impl CodexSessionHistorySanitizeSummary {
    pub fn changed_anything(&self) -> bool {
        self.updated_item_count > 0
    }
}

/// 清理指定实例目录下所有历史库里的第三方 reasoning 项（幂等）。
pub fn sanitize_official_incompatible_reasoning_history(
    data_dir: &Path,
) -> Result<CodexSessionHistorySanitizeSummary, String> {
    let _guard = HISTORY_SANITIZE_LOCK
        .lock()
        .map_err(|_| "会话历史清洗锁已中毒".to_string())?;

    let databases = history_database_paths(data_dir);
    let mut summary = CodexSessionHistorySanitizeSummary {
        database_count: databases.len(),
        ..Default::default()
    };
    if databases.is_empty() {
        return Ok(summary);
    }

    let mut backup_root: Option<PathBuf> = None;
    for database in databases {
        let (updated_items, changed_threads) =
            sanitize_history_database(data_dir, &database, &mut backup_root)?;
        summary.updated_item_count += updated_items;
        summary.changed_thread_count += changed_threads;
    }
    // 全部备份和写入成功后才轮换，失败的尝试不能淘汰已有的可恢复副本。
    if backup_root.is_some() {
        prune_backup_dirs(data_dir);
    }
    Ok(summary)
}

/// 历史库候选：实例根目录与 `sqlite/` 子目录下的 `thread_history_*.sqlite`。
fn history_database_paths(data_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_history_databases(data_dir, &mut paths);
    collect_history_databases(&data_dir.join(SQLITE_DIR_NAME), &mut paths);
    paths.sort();
    paths.dedup();
    paths
}

fn collect_history_databases(dir: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let matches_stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.starts_with(HISTORY_DB_STEM));
        let matches_extension =
            path.extension().and_then(|extension| extension.to_str()) == Some(HISTORY_DB_EXTENSION);
        if matches_stem && matches_extension {
            paths.push(path);
        }
    }
}

fn sanitize_history_database(
    data_dir: &Path,
    database: &Path,
    backup_root: &mut Option<PathBuf>,
) -> Result<(usize, usize), String> {
    let connection = Connection::open(database)
        .map_err(|error| format!("打开会话历史库失败 ({}): {}", database.display(), error))?;
    connection
        .busy_timeout(Duration::from_secs(BUSY_TIMEOUT_SECONDS))
        .map_err(|error| {
            format!(
                "设置会话历史库 busy_timeout 失败 ({}): {}",
                database.display(),
                error
            )
        })?;

    let pending = collect_sanitize_updates(&connection, database)?;
    if pending.is_empty() {
        return Ok((0, 0));
    }

    let root = match backup_root {
        Some(root) => root.clone(),
        None => {
            let root = create_backup_root(data_dir)?;
            *backup_root = Some(root.clone());
            root
        }
    };
    backup_history_database(&connection, data_dir, database, &root)?;

    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("开启会话历史库事务失败 ({}): {}", database.display(), error))?;
    for update in &pending {
        transaction
            .execute(
                "UPDATE thread_items SET item_json = ?1 WHERE rowid = ?2",
                rusqlite::params![update.item_json, update.rowid],
            )
            .map_err(|error| {
                format!(
                    "写入会话历史项失败 ({} rowid={}): {}",
                    database.display(),
                    update.rowid,
                    error
                )
            })?;
    }
    transaction
        .commit()
        .map_err(|error| format!("提交会话历史库事务失败 ({}): {}", database.display(), error))?;

    let changed_threads = pending
        .iter()
        .map(|update| update.thread_id.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    modules::logger::log_info(&format!(
        "[Codex History Sanitize] 已清理第三方推理历史: database={}, updated_items={}, changed_threads={}",
        database.display(),
        pending.len(),
        changed_threads
    ));
    Ok((pending.len(), changed_threads))
}

struct HistoryItemUpdate {
    rowid: i64,
    thread_id: String,
    item_json: String,
}

/// 收集需要清理的行；已经是 `content: []` 的行会被跳过，保证幂等。
fn collect_sanitize_updates(
    connection: &Connection,
    database: &Path,
) -> Result<Vec<HistoryItemUpdate>, String> {
    let mut updates = Vec::new();
    for table in ITEM_TABLES {
        if !table_exists(connection, table)? {
            continue;
        }
        if table != "thread_items" {
            // 其它表结构与 thread_items 不同（无 thread_id 语义），当前只处理主历史表。
            continue;
        }
        let mut statement = connection
            .prepare(&format!(
                "SELECT rowid, thread_id, item_json FROM {table} WHERE item_json LIKE '%\"reasoning\"%'"
            ))
            .map_err(|error| {
                format!(
                    "查询会话历史项失败 ({} / {}): {}",
                    database.display(),
                    table,
                    error
                )
            })?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|error| {
                format!(
                    "读取会话历史项失败 ({} / {}): {}",
                    database.display(),
                    table,
                    error
                )
            })?;
        for row in rows {
            let (rowid, thread_id, item_json) = row.map_err(|error| {
                format!(
                    "解析会话历史项失败 ({} / {}): {}",
                    database.display(),
                    table,
                    error
                )
            })?;
            let Some(sanitized) = sanitize_history_item_json(&item_json) else {
                continue;
            };
            updates.push(HistoryItemUpdate {
                rowid,
                thread_id,
                item_json: sanitized,
            });
        }
    }
    Ok(updates)
}

/// 把第三方 reasoning 项的 `content` 清空；官方形状（空数组）原样返回 `None`。
fn sanitize_history_item_json(item_json: &str) -> Option<String> {
    let mut value: Value = serde_json::from_str(item_json).ok()?;
    let object = value.as_object_mut()?;
    if object.get("type").and_then(Value::as_str) != Some("reasoning") {
        return None;
    }
    let has_visible_content = object
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|content| !content.is_empty());
    if !has_visible_content {
        return None;
    }
    object.insert("content".to_string(), Value::Array(Vec::new()));
    serde_json::to_string(&value).ok()
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, String> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .map_err(|error| format!("读取 SQLite 表结构失败 (table={}): {}", table, error))?;
    Ok(count > 0)
}

fn create_backup_root(data_dir: &Path) -> Result<PathBuf, String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let root = data_dir
        .join(BACKUP_DIR_NAME)
        .join(format!("sanitize-{timestamp}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root)
        .map_err(|error| format!("创建会话历史备份目录失败 ({}): {}", root.display(), error))?;
    Ok(root)
}

fn prune_backup_dirs(data_dir: &Path) {
    let backup_root = data_dir.join(BACKUP_DIR_NAME);
    let Ok(entries) = fs::read_dir(&backup_root) else {
        return;
    };
    let mut dirs = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("sanitize-"))
        })
        .collect::<Vec<_>>();
    if dirs.len() <= MAX_BACKUP_DIRS {
        return;
    }
    dirs.sort();
    let remove_count = dirs.len() - MAX_BACKUP_DIRS;
    for path in dirs.into_iter().take(remove_count) {
        let _ = fs::remove_dir_all(path);
    }
}

fn backup_history_database(
    connection: &Connection,
    data_dir: &Path,
    database: &Path,
    backup_root: &Path,
) -> Result<(), String> {
    // 根目录与 sqlite/ 可包含同名库，备份必须保留相对目录，不能互相覆盖。
    let relative = database.strip_prefix(data_dir).map_err(|error| {
        format!(
            "会话历史库不在实例目录内 ({}): {}",
            database.display(),
            error
        )
    })?;
    let target = backup_root.join(relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("创建会话历史备份目录失败 ({}): {}", parent.display(), error)
        })?;
    }
    // VACUUM INTO 拒绝非空的已有目标；保留旧备份并中止，不做覆盖删除。
    let target_text = target.to_string_lossy().to_string();
    connection
        .execute("VACUUM main INTO ?1", [target_text.as_str()])
        .map_err(|error| {
            format!(
                "备份会话历史库失败 ({} -> {}): {}",
                database.display(),
                target.display(),
                error
            )
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), unique));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("cleanup");
        }
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn create_history_db(dir: &Path) -> PathBuf {
        let path = dir.join("thread_history_1.sqlite");
        let connection = Connection::open(&path).expect("open db");
        connection
            .execute_batch(
                "CREATE TABLE thread_items (
                    thread_id TEXT NOT NULL,
                    turn_id TEXT NOT NULL,
                    item_id TEXT NOT NULL,
                    rollout_ordinal INTEGER NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    item_json TEXT NOT NULL,
                    item_type TEXT NOT NULL DEFAULT '',
                    updated_at_ordinal INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (thread_id, turn_id, item_id)
                );",
            )
            .expect("create table");
        path
    }

    fn insert_item(connection: &Connection, thread: &str, item_id: &str, item_json: &str) {
        connection
            .execute(
                "INSERT INTO thread_items (thread_id, turn_id, item_id, rollout_ordinal, created_at_ms, item_json, item_type)
                 VALUES (?1, 'turn-1', ?2, 0, 0, ?3, 'reasoning')",
                rusqlite::params![thread, item_id, item_json],
            )
            .expect("insert item");
    }

    #[test]
    fn clears_third_party_reasoning_content_and_is_idempotent() {
        let dir = make_temp_dir("codex-history-sanitize-test");
        let db = create_history_db(&dir);
        let connection = Connection::open(&db).expect("open db");
        insert_item(
            &connection,
            "thread-1",
            "item-1",
            r#"{"type":"reasoning","id":"r1","summary":[],"content":["thinking text"]}"#,
        );
        insert_item(
            &connection,
            "thread-1",
            "item-2",
            r#"{"type":"reasoning","id":"r2","summary":["done"],"content":[]}"#,
        );
        insert_item(
            &connection,
            "thread-2",
            "item-3",
            r#"{"type":"agentMessage","id":"m1","text":"hello"}"#,
        );
        drop(connection);

        let first = sanitize_official_incompatible_reasoning_history(&dir).expect("sanitize");
        assert_eq!(first.database_count, 1);
        assert_eq!(first.updated_item_count, 1);
        assert_eq!(first.changed_thread_count, 1);

        let connection = Connection::open(&db).expect("reopen db");
        let updated: String = connection
            .query_row(
                "SELECT item_json FROM thread_items WHERE item_id = 'item-1'",
                [],
                |row| row.get(0),
            )
            .expect("read updated item");
        assert_eq!(
            updated,
            r#"{"content":[],"id":"r1","summary":[],"type":"reasoning"}"#
        );
        let untouched: String = connection
            .query_row(
                "SELECT item_json FROM thread_items WHERE item_id = 'item-3'",
                [],
                |row| row.get(0),
            )
            .expect("read untouched item");
        assert!(untouched.contains("agentMessage"));
        drop(connection);

        let second =
            sanitize_official_incompatible_reasoning_history(&dir).expect("sanitize again");
        assert_eq!(second.updated_item_count, 0);
        assert!(!second.changed_anything());

        let backup_root = dir.join(BACKUP_DIR_NAME);
        let backup_files = fs::read_dir(&backup_root)
            .expect("backup root")
            .flatten()
            .flat_map(|entry| fs::read_dir(entry.path()).into_iter().flatten().flatten())
            .count();
        assert_eq!(backup_files, 1, "应生成一份历史库备份");
    }

    #[test]
    fn ignores_directories_without_history_databases() {
        let dir = make_temp_dir("codex-history-sanitize-empty");
        let summary = sanitize_official_incompatible_reasoning_history(&dir).expect("sanitize");
        assert_eq!(summary.database_count, 0);
        assert_eq!(summary.updated_item_count, 0);
    }

    #[test]
    fn preserves_backups_for_same_named_databases_in_both_locations() {
        let dir = make_temp_dir("codex-history-sanitize-backup-collision");
        let sqlite_dir = dir.join(SQLITE_DIR_NAME);
        fs::create_dir_all(&sqlite_dir).expect("create sqlite dir");
        for (location, thread) in [(&dir, "root-thread"), (&sqlite_dir, "sqlite-thread")] {
            let db = create_history_db(location);
            let connection = Connection::open(db).expect("open db");
            insert_item(
                &connection,
                thread,
                "item-1",
                r#"{"type":"reasoning","summary":[],"content":["original thinking"]}"#,
            );
        }

        let summary = sanitize_official_incompatible_reasoning_history(&dir).expect("sanitize");
        assert_eq!(summary.database_count, 2);
        assert_eq!(summary.updated_item_count, 2);
        let backup = fs::read_dir(dir.join(BACKUP_DIR_NAME))
            .expect("backup parent")
            .next()
            .expect("one backup")
            .expect("backup entry")
            .path();
        for (relative, thread) in [
            ("thread_history_1.sqlite", "root-thread"),
            ("sqlite/thread_history_1.sqlite", "sqlite-thread"),
        ] {
            assert!(
                backup.join(relative).is_file(),
                "missing backup: {relative}"
            );
            let connection = Connection::open(backup.join(relative)).expect("open backup");
            let (saved_thread, saved_json): (String, String) = connection
                .query_row("SELECT thread_id, item_json FROM thread_items", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .expect("read backup");
            assert_eq!(saved_thread, thread);
            assert!(saved_json.contains("original thinking"));
        }
    }

    #[test]
    fn backup_collision_leaves_existing_backup_and_source_unchanged() {
        let dir = make_temp_dir("codex-history-sanitize-backup-failure");
        let db = create_history_db(&dir);
        let connection = Connection::open(&db).expect("open db");
        let original = r#"{"type":"reasoning","content":["original thinking"]}"#;
        insert_item(&connection, "thread-1", "item-1", original);
        let backup = create_backup_root(&dir).expect("create backup root");
        let target = backup.join(db.file_name().expect("db name"));
        fs::write(&target, "existing backup").expect("seed backup collision");

        assert!(sanitize_history_database(&dir, &db, &mut Some(backup)).is_err());
        assert_eq!(
            fs::read_to_string(target).expect("existing backup"),
            "existing backup"
        );
        let saved: String = connection
            .query_row("SELECT item_json FROM thread_items", [], |row| row.get(0))
            .expect("read source");
        assert_eq!(saved, original);
    }

    #[test]
    fn backup_roots_are_unique_and_do_not_prune_before_success() {
        let dir = make_temp_dir("codex-history-sanitize-backup-retention");
        let parent = dir.join(BACKUP_DIR_NAME);
        for index in 0..MAX_BACKUP_DIRS {
            fs::create_dir_all(parent.join(format!("sanitize-000{index}"))).expect("old backup");
        }
        let unrelated = parent.join("manual-backup");
        fs::create_dir_all(&unrelated).expect("unrelated directory");
        let first = create_backup_root(&dir).expect("first backup root");
        let second = create_backup_root(&dir).expect("second backup root");
        assert_ne!(first, second);
        for index in 0..MAX_BACKUP_DIRS {
            assert!(parent.join(format!("sanitize-000{index}")).is_dir());
        }

        prune_backup_dirs(&dir);
        assert!(
            unrelated.is_dir(),
            "only managed backup directories may be pruned"
        );
        assert!(first.is_dir());
        assert!(second.is_dir());
        assert_eq!(
            fs::read_dir(parent).expect("backup parent").count(),
            MAX_BACKUP_DIRS + 1
        );
    }

    #[test]
    fn sanitizes_items_inside_sqlite_subdirectory() {
        let dir = make_temp_dir("codex-history-sanitize-subdir");
        let sqlite_dir = dir.join(SQLITE_DIR_NAME);
        fs::create_dir_all(&sqlite_dir).expect("create sqlite dir");
        let db = sqlite_dir.join("thread_history_2.sqlite");
        let connection = Connection::open(&db).expect("open db");
        connection
            .execute_batch(
                "CREATE TABLE thread_items (
                    thread_id TEXT NOT NULL,
                    turn_id TEXT NOT NULL,
                    item_id TEXT NOT NULL,
                    rollout_ordinal INTEGER NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    item_json TEXT NOT NULL,
                    item_type TEXT NOT NULL DEFAULT '',
                    updated_at_ordinal INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (thread_id, turn_id, item_id)
                );",
            )
            .expect("create table");
        insert_item(
            &connection,
            "thread-9",
            "item-9",
            r#"{"type":"reasoning","id":"r9","summary":[],"content":[{"type":"reasoning_text","text":"x"}]}"#,
        );
        drop(connection);

        let summary = sanitize_official_incompatible_reasoning_history(&dir).expect("sanitize");
        assert_eq!(summary.updated_item_count, 1);
        let connection = Connection::open(&db).expect("reopen");
        let updated: String = connection
            .query_row(
                "SELECT item_json FROM thread_items WHERE item_id = 'item-9'",
                [],
                |row| row.get(0),
            )
            .expect("read item");
        assert!(updated.contains(r#""content":[]"#));
    }
}
