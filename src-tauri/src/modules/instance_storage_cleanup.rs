//! Explicit, fail-closed cleanup of unregistered managed instance directories.
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::modules;

static CLEANUP_OPERATION: Mutex<()> = Mutex::new(());
static INSTANCE_CREATION: RwLock<()> = RwLock::new(());

/// Creation/import never waits for a cleanup operation on the UI path.
pub fn protect_instance_creation() -> Result<RwLockReadGuard<'static, ()>, String> {
    INSTANCE_CREATION
        .try_read()
        .map_err(|_| "Instance storage cleanup is busy; please retry".into())
}

#[derive(Debug, Serialize)]
pub struct OrphanInstanceDir {
    pub path: String,
    pub platform: String,
    pub bytes: Option<u64>,
    #[serde(rename = "sizeError")]
    pub size_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CleanupFailure {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Default, Serialize)]
pub struct CleanupResult {
    pub deleted: Vec<String>,
    pub failed: Vec<CleanupFailure>,
}

struct PlatformRoot {
    platform: String,
    root: PathBuf,
}

#[derive(Deserialize)]
struct Registry {
    instances: Vec<RegistryEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryEntry {
    user_data_dir: String,
}

struct Protection {
    paths: Vec<PathBuf>,
    process_text: Vec<String>,
    runtime_paths: Vec<PathBuf>,
}

fn path_key(path: &Path) -> PathBuf {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    {
        let value = path.to_string_lossy().replace('/', "\\").to_lowercase();
        let value = if let Some(value) = value.strip_prefix("\\\\?\\unc\\") {
            format!("\\\\{}", value)
        } else {
            value.strip_prefix("\\\\?\\").unwrap_or(&value).to_string()
        };
        return PathBuf::from(value);
    }
    #[cfg(not(windows))]
    path
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn app_data_hash(home: &Path) -> String {
    // Match the desktop launcher's exact formula. Unlike comparison keys, the
    // Windows canonical extended prefix is intentionally retained in this hash.
    let resolved = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let normalized = resolved.to_string_lossy().to_string();
    #[cfg(windows)]
    let normalized = normalized.replace('/', "\\").to_lowercase();
    format!("{:x}", md5::compute(normalized.as_bytes()))
}

fn process_paths(values: &[String]) -> Vec<PathBuf> {
    let paths: HashSet<_> = values
        .iter()
        .filter_map(|value| {
            let value = value
                .split_once('=')
                .map(|(_, value)| value)
                .unwrap_or(value);
            let value = value.trim_matches(|character| character == '\'' || character == '"');
            Path::new(value).is_absolute().then(|| PathBuf::from(value))
        })
        .collect();
    paths.iter().map(|path| path_key(path)).collect()
}

fn read_registry(path: &Path, protected: &mut Vec<PathBuf>) -> Result<(), String> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let registry: Registry = serde_json::from_str(&content).map_err(|e| {
                format!(
                    "Cannot safely read instance registry {}: {}",
                    path.display(),
                    e
                )
            })?;
            for entry in registry.instances {
                let value = entry.user_data_dir.trim();
                if value.is_empty() || !Path::new(value).is_absolute() {
                    return Err(format!("Invalid instance directory in {}", path.display()));
                }
                protected.push(path_key(Path::new(value)));
                // Keep both spellings: symlinks may disappear between scans.
                protected.push(PathBuf::from(value));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Cannot read {}: {}", path.display(), e)),
    }
}

fn roots_and_defaults() -> Result<(Vec<PlatformRoot>, Vec<PathBuf>), String> {
    let mut roots = Vec::new();
    let mut defaults = Vec::new();
    macro_rules! platform {
        ($name:expr, $module:ident) => {{
            let value = modules::$module::get_instance_defaults()?;
            roots.push(PlatformRoot {
                platform: $name.into(),
                root: PathBuf::from(value.root_dir),
            });
            defaults.push(path_key(Path::new(&value.default_user_data_dir)));
        }};
    }
    platform!("antigravity", instance);
    platform!("antigravity-legacy", antigravity_legacy_instance);
    platform!("codex", codex_instance);
    platform!("claude", claude_instance);
    platform!("github_copilot", github_copilot_instance);
    platform!("windsurf", windsurf_instance);
    platform!("cursor", cursor_instance);
    platform!("kiro", kiro_instance);
    platform!("codebuddy", codebuddy_instance);
    platform!("codebuddy_cn", codebuddy_cn_instance);
    platform!("qoder", qoder_instance);
    platform!("workbuddy", workbuddy_instance);
    platform!("zcode", zcode_instance);
    platform!("grok", grok_instance);
    use modules::trae_account::TraePlatformKind;
    for (name, kind) in [
        ("trae", TraePlatformKind::Trae),
        ("trae-solo", TraePlatformKind::TraeSolo),
        ("trae-cn", TraePlatformKind::TraeCn),
        ("trae-solo-cn", TraePlatformKind::TraeSoloCn),
    ] {
        roots.push(PlatformRoot {
            platform: name.into(),
            root: modules::trae_instance::get_default_instances_root_dir_for_platform(kind)?,
        });
        defaults.push(path_key(
            &modules::trae_instance::get_default_trae_user_data_dir_for_platform(kind)?,
        ));
    }
    defaults.push(path_key(
        &modules::claude_instance::get_default_claude_cli_config_dir()?,
    ));
    // The host itself can inherit a managed CODEX_HOME; the official desktop's
    // default home remains protected independently of that environment override.
    if let Some(home) = dirs::home_dir() {
        defaults.push(path_key(&home.join(".codex")));
    }
    roots.push(PlatformRoot {
        platform: "codex-app-data".into(),
        root: modules::account::get_data_dir()?.join("instances/codex-app-data"),
    });
    // Several legacy platform getters still resolve production paths even in a dev
    // process. Never use a dev registry to authorize deletion in production data.
    let data = modules::account::get_data_dir()?;
    roots.retain(|root| root_is_in_data_dir(root, &data));
    Ok((roots, defaults))
}

fn root_is_in_data_dir(root: &PlatformRoot, data: &Path) -> bool {
    root.root.parent().map(path_key) == Some(path_key(&data.join("instances")))
}

fn read_recovery_paths(path: &Path, protected: &mut Vec<PathBuf>) -> Result<(), String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(format!(
                "Cannot read recovery state {}: {}",
                path.display(),
                e
            ))
        }
    };
    #[derive(Deserialize)]
    struct RecoveryState {
        profiles: Vec<RecoveryProfile>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RecoveryProfile {
        profile_dir: String,
    }
    let state: RecoveryState = serde_json::from_str(&content)
        .map_err(|e| format!("Invalid recovery state {}: {}", path.display(), e))?;
    for profile in state.profiles {
        let path = Path::new(profile.profile_dir.trim());
        if !path.is_absolute() {
            return Err("Invalid profile directory in recovery state".into());
        }
        protected.push(path_key(path));
    }
    Ok(())
}

fn process_references() -> Result<Vec<String>, String> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
    if system.processes().is_empty() {
        return Err("Cannot inspect running processes safely".into());
    }
    let mut refs = Vec::new();
    for process in system.processes().values() {
        let name = process.name().to_string_lossy().to_lowercase();
        let relevant = [
            "codex",
            "claude",
            "antigravity",
            "windsurf",
            "cursor",
            "kiro",
            "trae",
            "codebuddy",
            "workbuddy",
            "qoder",
            "zcode",
            "grok",
        ]
        .iter()
        .any(|item| name.contains(item));
        if relevant && process.cmd().is_empty() && process.cwd().is_none() {
            return Err(format!("Cannot safely inspect running client {}", name));
        }
        for value in process.cmd().iter().chain(process.environ()) {
            refs.push(value.to_string_lossy().into_owned());
        }
        if let Some(cwd) = process.cwd() {
            refs.push(cwd.to_string_lossy().into_owned());
        }
    }
    #[cfg(windows)]
    {
        refs = refs
            .into_iter()
            .map(|value| value.replace('/', "\\").to_lowercase())
            .collect();
    }
    Ok(refs)
}

fn collect_protection(
    data: &Path,
    roots: &[PlatformRoot],
    defaults: Vec<PathBuf>,
    process_text: Vec<String>,
) -> Result<Protection, String> {
    let runtime_paths = process_paths(&process_text);
    collect_protection_with_runtime(data, roots, defaults, process_text, runtime_paths)
}

fn collect_protection_with_runtime(
    data: &Path,
    roots: &[PlatformRoot],
    defaults: Vec<PathBuf>,
    process_text: Vec<String>,
    runtime_paths: Vec<PathBuf>,
) -> Result<Protection, String> {
    let mut paths: Vec<_> = defaults.iter().map(|path| path_key(path)).collect();
    // Root scanning is an explicit platform allowlist, but references also include
    // legacy or future registries: any platform may register a custom shared path.
    for entry in fs::read_dir(data).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let base = name.strip_suffix(".bak").unwrap_or(&name);
        if base == "instances.json" || base.ends_with("_instances.json") {
            read_registry(&entry.path(), &mut paths)?;
        }
        if let Some((original, _)) = name.split_once(".invalid-json.") {
            if original == "instances.json"
                || original.ends_with("_instances.json")
                || original == "codex_local_access_takeover_backups.json"
            {
                return Err(format!(
                    "Quarantined instance or recovery registry requires recovery: {}",
                    name
                ));
            }
        }
    }
    for name in [
        "codex_local_access_takeover_backups.json",
        "codex_local_access_takeover_backups.json.bak",
    ] {
        read_recovery_paths(&data.join(name), &mut paths)?;
    }
    if let Some(app_root) = roots.iter().find(|root| root.platform == "codex-app-data") {
        // Hash every registered/default path conservatively; this also covers custom Codex homes.
        let mut homes = paths.clone();
        for value in &process_text {
            if let Some(home) = value
                .strip_prefix("CODEX_HOME=")
                .or_else(|| value.strip_prefix("codex_home="))
            {
                homes.push(PathBuf::from(home));
            }
        }
        for root in roots.iter().filter(|root| root.platform == "codex") {
            if root.root.is_dir() && !has_link_ancestor(&root.root)? {
                for entry in fs::read_dir(&root.root).map_err(|error| error.to_string())? {
                    let entry = entry.map_err(|error| error.to_string())?;
                    let key = path_key(&entry.path());
                    let app_path = path_key(&app_root.root.join(app_data_hash(&entry.path())));
                    if process_text.iter().any(|value| {
                        value.contains(key.to_string_lossy().as_ref())
                            || value.contains(app_path.to_string_lossy().as_ref())
                    }) || runtime_paths
                        .iter()
                        .any(|path| path.starts_with(&key) || path.starts_with(&app_path))
                    {
                        homes.push(entry.path());
                        paths.push(key);
                    }
                }
            }
        }
        for home in homes {
            paths.push(path_key(&app_root.root.join(app_data_hash(&home))));
        }
    }
    Ok(Protection {
        paths,
        process_text,
        runtime_paths,
    })
}

fn protected(path: &Path, protection: &Protection) -> bool {
    let key = path_key(path);
    if protection.paths.iter().any(|saved| overlaps(&key, saved)) {
        return true;
    }
    // A process working in HOME (or exporting HOME) does not own every child
    // directory. Only references to this candidate or its contents protect it.
    if protection
        .runtime_paths
        .iter()
        .any(|path| path.starts_with(&key))
    {
        return true;
    }
    let text = key.to_string_lossy();
    protection
        .process_text
        .iter()
        .any(|value| value.contains(text.as_ref()))
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions and all other reparse points must be excluded too.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn has_link_ancestor(path: &Path) -> Result<bool, String> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_link(&metadata) => return Ok(true),
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(false)
}

#[derive(Serialize, Deserialize)]
struct CleanupJournal {
    original: PathBuf,
}

fn journal_path(path: &Path) -> PathBuf {
    path.with_extension("cleanup.json")
}

fn read_cleanup_journal(path: &Path) -> Result<Option<CleanupJournal>, String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let Some(id) = name.strip_prefix(".cleanup-") else {
        return Ok(None);
    };
    if uuid::Uuid::parse_str(id).is_err() {
        return Ok(None);
    }
    let journal_path = journal_path(path);
    let metadata = fs::symlink_metadata(&journal_path).map_err(|error| error.to_string())?;
    if is_link(&metadata) || !metadata.is_file() || metadata.len() > 16_384 {
        return Err(format!(
            "Invalid cleanup recovery record: {}",
            journal_path.display()
        ));
    }
    let journal: CleanupJournal = serde_json::from_str(
        &fs::read_to_string(&journal_path).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if journal.original.parent() != path.parent()
        || journal.original.file_name().is_none()
        || journal
            .original
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with('.')
    {
        return Err("Invalid cleanup recovery directory".into());
    }
    Ok(Some(journal))
}

fn candidate_paths(
    roots: &[PlatformRoot],
    protection: &Protection,
) -> Result<Vec<(String, PathBuf)>, String> {
    let mut result = Vec::new();
    for root in roots {
        let metadata = match fs::symlink_metadata(&root.root) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        if !metadata.is_dir() || has_link_ancestor(&root.root)? {
            continue;
        }
        for entry in fs::read_dir(&root.root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if !metadata.is_dir() || is_link(&metadata) {
                continue;
            }
            if entry.file_name().to_string_lossy().starts_with('.') {
                match read_cleanup_journal(&path)? {
                    Some(journal) if !protected(&journal.original, protection) => (),
                    _ => continue,
                }
            }
            if !protected(&path, protection) {
                result.push((root.platform.clone(), path));
            }
        }
    }
    Ok(result)
}

fn directory_bytes(path: &Path, deadline: Instant) -> Result<u64, String> {
    let mut pending = vec![path.to_path_buf()];
    let mut bytes = 0u64;
    let mut visited = 0usize;
    while let Some(path) = pending.pop() {
        if Instant::now() > deadline {
            return Err("Instance storage scan timed out; retry with fewer files".into());
        }
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
            if is_link(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    bytes = bytes.saturating_add(metadata.blocks().saturating_mul(512));
                }
                #[cfg(not(unix))]
                {
                    bytes = bytes.saturating_add(metadata.len());
                }
            }
            visited += 1;
            if visited > 2_000_000 {
                return Err("Instance storage scan file limit exceeded".into());
            }
            if visited % 256 == 0 {
                std::thread::yield_now();
                if Instant::now() > deadline {
                    return Err("Instance storage scan timed out".into());
                }
            }
        }
    }
    Ok(bytes)
}

pub fn scan_orphan_instance_dirs() -> Result<Vec<OrphanInstanceDir>, String> {
    let _operation = CLEANUP_OPERATION
        .try_lock()
        .map_err(|_| "Instance storage cleanup is busy; please retry")?;
    let (roots, defaults) = roots_and_defaults()?;
    let protection = collect_protection(
        &modules::account::get_data_dir()?,
        &roots,
        defaults,
        process_references()?,
    )?;
    modules::logger::log_info("[Instance cleanup] Manual scan started");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut rows = Vec::new();
    for (platform, path) in candidate_paths(&roots, &protection)? {
        let (bytes, size_error) = match directory_bytes(&path, deadline) {
            Ok(bytes) => (Some(bytes), None),
            Err(error) => {
                modules::logger::log_warn(&format!(
                    "[Instance cleanup] Size scan failed: path={}, error={}",
                    path.display(),
                    error
                ));
                (None, Some(error))
            }
        };
        rows.push(OrphanInstanceDir {
            bytes,
            size_error,
            path: path.to_string_lossy().into_owned(),
            platform,
        });
    }
    rows.sort_by(|left, right| right.bytes.cmp(&left.bytes));
    modules::logger::log_info(&format!(
        "[Instance cleanup] Manual scan completed: candidates={}",
        rows.len()
    ));
    Ok(rows)
}

fn remove_directory_bounded(
    path: &Path,
    deadline: Instant,
    max_entries: usize,
) -> Result<(), String> {
    let mut pending = vec![(path.to_path_buf(), false)];
    let mut visited = 0usize;
    while let Some((directory, remove)) = pending.pop() {
        if Instant::now() > deadline {
            return Err("Cleanup time limit reached; retry to continue".into());
        }
        if remove {
            fs::remove_dir(&directory).map_err(|error| error.to_string())?;
            continue;
        }
        let metadata = fs::symlink_metadata(&directory).map_err(|error| error.to_string())?;
        if is_link(&metadata) {
            return Err("Directory changed into a link during cleanup".into());
        }
        pending.push((directory.clone(), true));
        for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
            if Instant::now() > deadline || visited >= max_entries {
                return Err("Cleanup work limit reached; retry to continue".into());
            }
            visited += 1;
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if metadata.is_dir() && !is_link(&metadata) {
                pending.push((entry.path(), false));
            } else {
                #[cfg(windows)]
                let removal = if metadata.is_dir() && is_link(&metadata) {
                    fs::remove_dir(entry.path())
                } else {
                    fs::remove_file(entry.path())
                };
                #[cfg(not(windows))]
                let removal = fs::remove_file(entry.path());
                removal.map_err(|error| error.to_string())?;
            }
            if visited % 256 == 0 {
                std::thread::yield_now();
            }
        }
    }
    Ok(())
}

struct CleanupContext {
    data: PathBuf,
    roots: Vec<PlatformRoot>,
    defaults: Vec<PathBuf>,
    process_text: Vec<String>,
}

pub fn delete_orphan_instance_dirs(paths: Vec<String>) -> Result<CleanupResult, String> {
    delete_selected(paths, || {
        let (roots, defaults) = roots_and_defaults()?;
        Ok(CleanupContext {
            data: modules::account::get_data_dir()?,
            roots,
            defaults,
            process_text: process_references()?,
        })
    })
}

fn delete_selected(
    paths: Vec<String>,
    mut snapshot: impl FnMut() -> Result<CleanupContext, String>,
) -> Result<CleanupResult, String> {
    let _operation = CLEANUP_OPERATION
        .try_lock()
        .map_err(|_| "Instance storage cleanup is busy; please retry")?;
    modules::logger::log_info(&format!(
        "[Instance cleanup] Manual deletion started: requested={}",
        paths.len()
    ));
    let mut result = CleanupResult::default();
    let mut seen = HashSet::new();
    for requested in paths {
        if !seen.insert(requested.clone()) {
            continue;
        }
        let outcome = (|| -> Result<(), String> {
            // Expensive platform/process probing stays outside the short creation gate.
            let CleanupContext {
                data,
                roots,
                defaults,
                process_text,
            } = snapshot()?;
            let runtime_paths = process_paths(&process_text);
            let guard = INSTANCE_CREATION
                .try_write()
                .map_err(|_| "Instance creation or import is in progress; please retry")?;
            let protection = collect_protection_with_runtime(
                &data,
                &roots,
                defaults,
                process_text,
                runtime_paths,
            )?;
            let candidates = candidate_paths(&roots, &protection)?;
            let path = candidates
                .into_iter()
                .map(|(_, path)| path)
                .find(|path| path.to_string_lossy() == requested)
                .ok_or("Directory is no longer an unreferenced managed instance")?;
            let (quarantine, original) = if let Some(journal) = read_cleanup_journal(&path)? {
                (path.clone(), journal.original)
            } else {
                let quarantine = path
                    .parent()
                    .ok_or("Missing parent directory")?
                    .join(format!(".cleanup-{}", uuid::Uuid::new_v4()));
                let journal = serde_json::to_vec(&CleanupJournal {
                    original: path.clone(),
                })
                .map_err(|error| error.to_string())?;
                // Persist recovery metadata before the atomic rename. Interrupted or partial
                // removals remain discoverable and retryable on the next manual scan.
                use std::io::Write;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(journal_path(&quarantine))
                    .map_err(|error| error.to_string())?;
                file.write_all(&journal)
                    .and_then(|_| file.sync_all())
                    .map_err(|error| error.to_string())?;
                fs::rename(&path, &quarantine)
                    .map_err(|e| format!("Cannot isolate directory: {}", e))?;
                (quarantine, path.clone())
            };
            drop(guard);
            if let Err(error) = remove_directory_bounded(
                &quarantine,
                Instant::now() + Duration::from_secs(30),
                250_000,
            ) {
                // Keep the journal and surviving data together, so retries cannot lose track
                // of partially deleted directories or overwrite a newly created profile.
                return Err(format!(
                    "Delete failed: {}; remaining data: {}; original directory: {}",
                    error,
                    quarantine.display(),
                    original.display()
                ));
            }
            fs::remove_file(journal_path(&quarantine)).map_err(|error| {
                format!(
                    "Data deleted; cannot remove cleanup recovery record: {}",
                    error
                )
            })?;
            Ok(())
        })();
        match outcome {
            Ok(()) => result.deleted.push(requested),
            Err(error) => {
                modules::logger::log_warn(&format!(
                    "[Instance cleanup] Deletion failed: path={}, error={}",
                    requested, error
                ));
                result.failed.push(CleanupFailure {
                    path: requested,
                    error,
                });
            }
        }
        std::thread::yield_now();
    }
    modules::logger::log_info(&format!(
        "[Instance cleanup] Manual deletion completed: deleted={}, failed={}",
        result.deleted.len(),
        result.failed.len()
    ));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("instance-cleanup-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
        fn root(&self) -> PlatformRoot {
            PlatformRoot {
                platform: "codex".into(),
                root: self.0.join("instances/codex"),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn registry_and_backup_protect_nested_paths() {
        let fixture = Fixture::new();
        let root = fixture.root();
        let live = root.root.join("live");
        let orphan = root.root.join("orphan");
        fs::create_dir_all(live.join("nested")).unwrap();
        fs::create_dir_all(&orphan).unwrap();
        fs::write(
            fixture.0.join("codex_instances.json.bak"),
            serde_json::json!({"instances":[{"userDataDir":live.join("nested") }]}).to_string(),
        )
        .unwrap();
        let protection = collect_protection(&fixture.0, &[fixture.root()], vec![], vec![]).unwrap();
        let rows = candidate_paths(&[root], &protection).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, orphan);
    }
    #[test]
    fn malformed_and_empty_registry_fail_closed() {
        let fixture = Fixture::new();
        for content in ["", "{", "{}", "{\"instances\":[{}]}"] {
            fs::write(fixture.0.join("codex_instances.json"), content).unwrap();
            assert!(collect_protection(&fixture.0, &[fixture.root()], vec![], vec![]).is_err());
        }
    }
    #[test]
    fn process_default_and_recovery_paths_are_protected() {
        let fixture = Fixture::new();
        let root = fixture.root();
        for name in ["running", "default", "recover", "orphan"] {
            fs::create_dir_all(root.root.join(name)).unwrap();
        }
        fs::write(
            fixture.0.join("codex_local_access_takeover_backups.json"),
            serde_json::json!({"profiles":[{"profileDir":root.root.join("recover")}]}).to_string(),
        )
        .unwrap();
        let protection = collect_protection(
            &fixture.0,
            &[fixture.root()],
            vec![root.root.join("default")],
            vec![format!(
                "CODEX_HOME={}",
                root.root.join("running").display()
            )],
        )
        .unwrap();
        let rows = candidate_paths(&[root], &protection).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].1.ends_with("orphan"));
    }
    #[test]
    fn app_data_hash_matches_registered_home() {
        let fixture = Fixture::new();
        let home = fixture.0.join("custom-home");
        fs::create_dir_all(&home).unwrap();
        let app_root = PlatformRoot {
            platform: "codex-app-data".into(),
            root: fixture.0.join("instances/codex-app-data"),
        };
        fs::create_dir_all(app_root.root.join(app_data_hash(&home))).unwrap();
        let protection = collect_protection(
            &fixture.0,
            &[
                fixture.root(),
                PlatformRoot {
                    platform: app_root.platform.clone(),
                    root: app_root.root.clone(),
                },
            ],
            vec![home],
            vec![],
        )
        .unwrap();
        assert!(candidate_paths(&[app_root], &protection)
            .unwrap()
            .is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_followed_or_counted() {
        let fixture = Fixture::new();
        let root = fixture.root();
        fs::create_dir_all(root.root.join("orphan")).unwrap();
        let outside = fixture.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("data"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.root.join("linked")).unwrap();
        std::os::unix::fs::symlink(&outside, root.root.join("orphan/link")).unwrap();
        let rows = candidate_paths(
            &[root],
            &Protection {
                runtime_paths: vec![],
                paths: vec![],
                process_text: vec![],
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            directory_bytes(&rows[0].1, Instant::now() + Duration::from_secs(1)).unwrap(),
            0
        );
        assert!(outside.join("data").exists());
    }
    #[test]
    fn creation_guard_is_nonblocking_and_exclusive_with_cleanup() {
        let lock = RwLock::new(());
        let creation = lock.try_read().unwrap();
        assert!(lock.try_write().is_err());
        drop(creation);
        let cleanup = lock.try_write().unwrap();
        assert!(lock.try_read().is_err());
        drop(cleanup);
    }
    #[test]
    fn dev_registry_cannot_authorize_production_cleanup() {
        let fixture = Fixture::new();
        let production = PlatformRoot {
            platform: "claude".into(),
            root: fixture.0.join("prod/instances/claude"),
        };
        let development = PlatformRoot {
            platform: "codex".into(),
            root: fixture.0.join("dev/instances/codex"),
        };
        assert!(!root_is_in_data_dir(&production, &fixture.0.join("dev")));
        assert!(root_is_in_data_dir(&development, &fixture.0.join("dev")));
    }
    #[test]
    fn interrupted_cleanup_is_discoverable_and_rechecks_original_reference() {
        let fixture = Fixture::new();
        let root = fixture.root();
        let original = root.root.join("original");
        let quarantine = root.root.join(format!(".cleanup-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&quarantine).unwrap();
        fs::write(
            journal_path(&quarantine),
            serde_json::to_vec(&CleanupJournal {
                original: original.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        let rows = candidate_paths(
            &[fixture.root()],
            &Protection {
                runtime_paths: vec![],
                paths: vec![],
                process_text: vec![],
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, quarantine);
        assert!(candidate_paths(
            &[fixture.root()],
            &Protection {
                runtime_paths: vec![],
                paths: vec![original],
                process_text: vec![]
            }
        )
        .unwrap()
        .is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn linked_managed_root_cannot_escape_to_other_data() {
        let fixture = Fixture::new();
        let outside = fixture.0.join("outside");
        fs::create_dir_all(outside.join("codex/profile")).unwrap();
        std::os::unix::fs::symlink(&outside, fixture.0.join("instances")).unwrap();
        assert!(candidate_paths(
            &[fixture.root()],
            &Protection {
                runtime_paths: vec![],
                paths: vec![],
                process_text: vec![]
            }
        )
        .unwrap()
        .is_empty());
    }
    #[test]
    fn desktop_app_data_process_reference_protects_codex_profile() {
        let fixture = Fixture::new();
        let root = fixture.root();
        let profile = root.root.join("running");
        fs::create_dir_all(&profile).unwrap();
        let app_root = PlatformRoot {
            platform: "codex-app-data".into(),
            root: fixture.0.join("instances/codex-app-data"),
        };
        let app_path = app_root.root.join(app_data_hash(&profile));
        fs::create_dir_all(&app_path).unwrap();
        let roots = [root, app_root];
        let protection = collect_protection(
            &fixture.0,
            &roots,
            vec![],
            vec![format!("--user-data-dir={}", app_path.display())],
        )
        .unwrap();
        assert!(candidate_paths(&roots, &protection).unwrap().is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn runtime_alias_paths_protect_real_profile() {
        let fixture = Fixture::new();
        let root = fixture.root();
        let profile = root.root.join("running");
        fs::create_dir_all(&profile).unwrap();
        let alias = fixture.0.join("alias");
        std::os::unix::fs::symlink(&profile, &alias).unwrap();
        for reference in [
            format!("CODEX_HOME={}", alias.display()),
            format!("CLAUDE_CONFIG_DIR={}", alias.display()),
            format!("--user-data-dir={}", alias.display()),
            alias.to_string_lossy().into_owned(),
        ] {
            let protection =
                collect_protection(&fixture.0, &[fixture.root()], vec![], vec![reference]).unwrap();
            assert!(candidate_paths(&[fixture.root()], &protection)
                .unwrap()
                .is_empty());
        }
    }
    #[test]
    fn invalid_recovery_schema_and_quarantine_fail_closed() {
        let fixture = Fixture::new();
        let path = fixture.0.join("codex_local_access_takeover_backups.json");
        for content in [
            "{}",
            "{\"profiles\":[{}]}",
            "{\"profiles\":[{\"profileDir\":\"relative\"}]}",
        ] {
            fs::write(&path, content).unwrap();
            assert!(collect_protection(&fixture.0, &[fixture.root()], vec![], vec![]).is_err());
        }
        fs::remove_file(path).unwrap();
        fs::write(
            fixture
                .0
                .join("codex_local_access_takeover_backups.json.invalid-json.1"),
            "{}",
        )
        .unwrap();
        assert!(collect_protection(&fixture.0, &[fixture.root()], vec![], vec![]).is_err());
    }
    #[test]
    fn deletion_budget_leaves_remaining_data_for_retry() {
        let fixture = Fixture::new();
        let target = fixture.0.join("target");
        fs::create_dir_all(&target).unwrap();
        for name in ["one", "two", "three"] {
            fs::write(target.join(name), name).unwrap();
        }
        assert!(
            remove_directory_bounded(&target, Instant::now() + Duration::from_secs(1), 1).is_err()
        );
        assert!(target.exists());
        remove_directory_bounded(&target, Instant::now() + Duration::from_secs(1), 10).unwrap();
        assert!(!target.exists());
    }
    #[test]
    fn process_home_and_ancestor_cwd_do_not_hide_orphans() {
        let fixture = Fixture::new();
        let root = fixture.root();
        fs::create_dir_all(root.root.join("orphan")).unwrap();
        let protection = collect_protection(
            &fixture.0,
            &[fixture.root()],
            vec![],
            vec![
                format!("HOME={}", fixture.0.display()),
                fixture.0.to_string_lossy().into_owned(),
            ],
        )
        .unwrap();
        assert_eq!(
            candidate_paths(&[fixture.root()], &protection)
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn batch_deletion_revalidates_protected_and_arbitrary_paths_and_retries_interruption() {
        let _env_guard = crate::modules::test_support::env_lock()
            .lock()
            .expect("environment lock");
        let fixture = Fixture::new();
        let root = fixture.root();
        let orphan = root.root.join("orphan");
        let live = root.root.join("live");
        let outside = fixture.0.join("outside");
        for path in [&orphan, &live, &outside] {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join("data"), "keep").unwrap();
        }
        // Simulate another action registering a candidate after the initial scan.
        assert_eq!(
            candidate_paths(
                &[fixture.root()],
                &Protection {
                    runtime_paths: vec![],
                    paths: vec![],
                    process_text: vec![]
                }
            )
            .unwrap()
            .len(),
            2
        );
        fs::write(
            fixture.0.join("codex_instances.json"),
            serde_json::json!({"instances":[{"userDataDir":live}]}).to_string(),
        )
        .unwrap();
        let original = root.root.join("interrupted");
        let quarantine = root.root.join(format!(".cleanup-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&quarantine).unwrap();
        fs::write(
            journal_path(&quarantine),
            serde_json::to_vec(&CleanupJournal { original }).unwrap(),
        )
        .unwrap();
        for name in ["first", "second"] {
            fs::write(quarantine.join(name), name).unwrap();
        }
        assert!(
            remove_directory_bounded(&quarantine, Instant::now() + Duration::from_secs(1), 1)
                .is_err()
        );
        let result = delete_selected(
            vec![
                orphan.to_string_lossy().into_owned(),
                live.to_string_lossy().into_owned(),
                outside.to_string_lossy().into_owned(),
                quarantine.to_string_lossy().into_owned(),
            ],
            || {
                Ok(CleanupContext {
                    data: fixture.0.clone(),
                    roots: vec![fixture.root()],
                    defaults: vec![],
                    process_text: vec![],
                })
            },
        )
        .unwrap();
        assert_eq!(result.deleted.len(), 2);
        assert_eq!(result.failed.len(), 2);
        assert!(!orphan.exists());
        assert!(live.join("data").exists());
        assert!(outside.join("data").exists());
        assert!(!quarantine.exists());
        assert!(!journal_path(&quarantine).exists());
    }
    #[test]
    fn another_platform_custom_profile_reference_protects_candidate() {
        let fixture = Fixture::new();
        let root = fixture.root();
        let profile = root.root.join("custom");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            fixture.0.join("claude_instances.json"),
            serde_json::json!({"instances":[{"userDataDir":profile}]}).to_string(),
        )
        .unwrap();
        let protection = collect_protection(&fixture.0, &[fixture.root()], vec![], vec![]).unwrap();
        assert!(candidate_paths(&[fixture.root()], &protection)
            .unwrap()
            .is_empty());
    }
}
