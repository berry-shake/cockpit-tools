//! Native OMP account management. OMP remains the only token refresher.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

static SETTINGS_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

mod accounts;
pub use accounts::{account_action, account_state, OmpAccountState};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmpSettings {
    pub executable: String,
    pub work_dir: String,
    pub selected_profile: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmpAccountSummary {
    provider: String,
    credential_type: String,
    email: Option<String>,
    account_id: Option<String>,
    org_name: Option<String>,
    disabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmpProfile {
    name: String,
    path: String,
    accounts: Vec<OmpAccountSummary>,
    warning: Option<String>,
    independent: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmpState {
    pub settings: OmpSettings,
    profiles: Vec<OmpProfile>,
    launch_supported: bool,
}

fn root_dir() -> Result<PathBuf, String> {
    Ok(dirs::home_dir().ok_or("无法定位用户目录")?.join(".omp"))
}

fn settings_path() -> Result<PathBuf, String> {
    Ok(super::account::get_data_dir()?.join("omp_launch.json"))
}

fn validate_profile(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
        && !name.ends_with('.');
    let base = name.split('.').next().unwrap_or("");
    let reserved = matches!(base, "con" | "prn" | "aux" | "nul")
        || ((base.starts_with("com") || base.starts_with("lpt"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit());
    if !valid || reserved {
        return Err("Profile 名称须为 1–64 位小写字母、数字、点、下划线或短横线，以字母或数字开头，不能以点结尾或使用系统保留名称".into());
    }
    Ok(())
}

fn profile_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    validate_profile(name)?;
    Ok(if name == "default" {
        root.to_path_buf()
    } else {
        root.join("profiles").join(name)
    })
}

/// Refuse symlinks: the profile shown in Cockpit must be the one we launch.
fn check_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            Err(format!("OMP 路径不是普通目录：{}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(format!("无法读取 OMP 目录：{}", path.display())),
    }
}

fn check_profile_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    check_directory(root)?;
    if name != "default" {
        check_directory(&root.join("profiles"))?;
    }
    let path = profile_path(root, name)?;
    check_directory(&path)?;
    check_directory(&path.join("agent"))?;
    Ok(path)
}

fn broker_configured(agent_dir: &Path) -> Result<bool, String> {
    for name in ["config.yml", "config.yaml"] {
        let raw = match fs::read_to_string(agent_dir.join(name)) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("无法读取 OMP 配置；未改动该文件".into()),
        };
        let config: serde_yaml::Value =
            serde_yaml::from_str(&raw).map_err(|_| "OMP 配置不是有效 YAML；未改动该文件")?;
        if config.is_null() {
            return Ok(false);
        }
        if !config.is_mapping() {
            return Err("OMP 配置必须是 YAML 对象".into());
        }
        let nested = config
            .get("auth")
            .and_then(|v| v.get("broker"))
            .and_then(|v| v.get("url"));
        let value = nested.or_else(|| config.get("auth.broker.url"));
        return Ok(value
            .and_then(|v| v.as_str())
            .is_some_and(|v| !v.trim().is_empty()));
    }
    Ok(false)
}

fn check_dotenv_overrides(path: &Path) -> Result<(), String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(format!("无法检查环境文件：{}；未启动 OMP", path.display())),
    };
    // OMP loads dotenv *after* profile bootstrap, and may restore keys cleared
    // from the parent shell. Reject routing overrides instead of launching a
    // different credential store or silently editing the user's dotenv files.
    static OVERRIDE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
        r"(?m)^\s*(?:export\s+)?(?:OMP_AUTH_BROKER_URL|(?:OMP|PI)_(?:PROFILE|CONFIG_DIR|CODING_AGENT_DIR)|XDG_(?:CONFIG|DATA|STATE|CACHE)_HOME)\s*="
    ).expect("static dotenv key pattern")
    });
    if OVERRIDE.is_match(&raw) {
        return Err(format!("{} 包含 profile、认证 broker 或存储路径覆盖。为避免登录到错误环境，未启动 OMP；请移除这些覆盖或选择独立工作目录。文件未被修改。", path.display()));
    }
    Ok(())
}

fn check_launch_env_files(home: &Path, profile: &Path, work: &Path) -> Result<(), String> {
    for path in [
        home.join(".env"),
        profile.join(".env"),
        profile.join("agent/.env"),
        work.join(".omp/.env"),
    ] {
        check_dotenv_overrides(&path)?;
    }
    // Bun can autoload .env, .env.local and NODE_ENV-specific variants.
    for entry in fs::read_dir(work).map_err(|_| "无法检查工作目录的环境配置")? {
        let entry = entry.map_err(|_| "无法检查工作目录的环境配置")?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name == ".env" || name.starts_with(".env.")) && entry.path().is_file() {
            check_dotenv_overrides(&entry.path())?;
        }
    }
    Ok(())
}

fn read_account_summaries(db_path: &Path) -> Result<Vec<OmpAccountSummary>, String> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    if fs::symlink_metadata(db_path)
        .map_err(|_| "无法检查 OMP 数据库")?
        .file_type()
        .is_symlink()
    {
        return Err("不读取符号链接指向的 OMP 数据库".into());
    }
    let db = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "无法只读打开 OMP 数据库")?;
    db.busy_timeout(std::time::Duration::from_millis(500))
        .map_err(|_| "OMP 数据库繁忙")?;
    let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='auth_credentials')", [], |r| r.get(0))
        .map_err(|_| "无法读取 OMP 数据库结构")?;
    if !exists {
        return Ok(Vec::new());
    }
    // Never select the whole credential JSON or return access/refresh/API keys to the UI.
    let mut stmt = db.prepare("SELECT provider, credential_type, json_extract(data, '$.email'), json_extract(data, '$.accountId'), json_extract(data, '$.orgName'), disabled_cause IS NOT NULL FROM auth_credentials WHERE provider NOT LIKE 'mcp_oauth:%' ORDER BY provider, id")
        .map_err(|_| "OMP 认证表版本不兼容；未修改数据库")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(OmpAccountSummary {
                provider: r.get(0)?,
                credential_type: r.get(1)?,
                email: r.get(2)?,
                account_id: r.get(3)?,
                org_name: r.get(4)?,
                disabled: r.get(5)?,
            })
        })
        .map_err(|_| "无法读取 OMP 账号摘要")?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "OMP 账号摘要格式不兼容；未修改数据库".into())
}

fn describe_profile(root: &Path, name: &str) -> OmpProfile {
    let path = profile_path(root, name).expect("validated profile name");
    let mut profile = OmpProfile {
        name: name.into(),
        path: path.to_string_lossy().into(),
        accounts: Vec::new(),
        warning: None,
        independent: false,
    };
    let inspected = (|| {
        check_profile_path(root, name)?;
        if broker_configured(&path.join("agent"))? {
            return Err("此 profile 使用远程认证 broker，不能作为独立登录环境启动。请新建 profile；现有配置不会被覆盖。".into());
        }
        profile.independent = true;
        profile.accounts = read_account_summaries(&path.join("agent/agent.db"))?;
        Ok::<_, String>(())
    })();
    if let Err(error) = inspected {
        profile.warning = Some(error);
    }
    profile
}

fn list_profiles(root: &Path) -> Result<Vec<OmpProfile>, String> {
    check_directory(root)?;
    check_directory(&root.join("profiles"))?;
    let mut names = Vec::new();
    match fs::read_dir(root.join("profiles")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|_| "无法枚举 OMP profiles")?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name != "default" && validate_profile(&name).is_ok() && entry.path().is_dir() {
                    names.push(name);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法枚举 OMP profiles".into()),
    }
    names.sort();
    names.insert(0, "default".into());
    Ok(names
        .iter()
        .map(|name| describe_profile(root, name))
        .collect())
}

fn executable_is_valid(path: &Path) -> bool {
    if !path.is_absolute() || !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn discover_executable() -> String {
    let mut candidates = Vec::new();
    if let Some(home) = dirs::home_dir() {
        for rel in [".local/bin/omp", ".bun/bin/omp", ".npm-global/bin/omp"] {
            candidates.push(home.join(rel));
        }
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/omp"),
        PathBuf::from("/usr/local/bin/omp"),
    ]);
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|p| p.join("omp")));
    }
    candidates
        .into_iter()
        .find(|p| executable_is_valid(p))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn load_settings() -> Result<OmpSettings, String> {
    match fs::read_to_string(settings_path()?) {
        Ok(raw) => {
            serde_json::from_str(&raw).map_err(|_| "OMP 启动设置损坏；未覆盖现有文件".into())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(OmpSettings {
            executable: discover_executable(),
            selected_profile: "default".into(),
            work_dir: String::new(),
        }),
        Err(_) => Err("无法读取 OMP 启动设置".into()),
    }
}

pub fn state() -> Result<OmpState, String> {
    Ok(OmpState {
        settings: load_settings()?,
        profiles: list_profiles(&root_dir()?)?,
        launch_supported: cfg!(target_os = "macos"),
    })
}

fn validate_settings(settings: &OmpSettings) -> Result<(), String> {
    validate_profile(&settings.selected_profile)?;
    if !settings.executable.is_empty() && !executable_is_valid(Path::new(&settings.executable)) {
        return Err("请选择有效的 OMP 可执行文件（绝对路径）".into());
    }
    if !settings.work_dir.is_empty()
        && (!Path::new(&settings.work_dir).is_absolute() || !Path::new(&settings.work_dir).is_dir())
    {
        return Err("工作目录必须是已存在目录的绝对路径".into());
    }
    Ok(())
}

pub fn save_settings(settings: OmpSettings) -> Result<(), String> {
    validate_settings(&settings)?;
    let _guard = SETTINGS_LOCK.lock().map_err(|_| "OMP 设置锁不可用")?;
    let raw = serde_json::to_string_pretty(&settings).map_err(|_| "无法序列化 OMP 设置")?;
    super::atomic_write::write_string_atomic(&settings_path()?, &raw)
}

fn create_profile_at(root: &Path, name: &str) -> Result<(), String> {
    if name == "default" {
        return Err("默认 profile 已保留，请使用其他名称".into());
    }
    let path = check_profile_path(root, name)?;
    // create_dir (not create_dir_all) for the leaf prevents overwriting existing profiles.
    fs::create_dir_all(root.join("profiles")).map_err(|_| "无法创建 OMP profiles 目录")?;
    fs::create_dir(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            "该 profile 已存在，未改动原有配置"
        } else {
            "无法创建 OMP profile"
        }
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "无法设置 OMP profile 权限")?;
    }
    // OMP creates/migrates its own agent.db on first login. Never initialize it here.
    Ok(())
}

pub fn create_profile(name: &str) -> Result<(), String> {
    create_profile_at(&root_dir()?, name)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn launch_command(settings: &OmpSettings, login: bool) -> String {
    // Pin the documented ~/.omp layout. Inherited profile/broker/XDG overrides
    // must not silently select a different credential store in a login shell.
    let cleared = [
        "OMP_PROFILE",
        "PI_PROFILE",
        "PI_CONFIG_DIR",
        "PI_CODING_AGENT_DIR",
        "OMP_AUTH_BROKER_URL",
        "OMP_AUTH_BROKER_TOKEN",
        "OMP_AUTH_BROKER_ACCOUNT_POOL_FILE",
        "OMP_AUTH_BROKER_SNAPSHOT_CACHE",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ];
    let env = cleared
        .iter()
        .map(|key| format!("-u {key}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut command = format!(
        "cd {} && /usr/bin/env {} {} --profile {}",
        shell_quote(&settings.work_dir),
        env,
        shell_quote(&settings.executable),
        shell_quote(&settings.selected_profile)
    );
    if login {
        command.push_str(" auth-broker login");
    } else {
        command.push_str(&format!(" --cwd {}", shell_quote(&settings.work_dir)));
    }
    command
}

pub fn launch(settings: OmpSettings, login: bool) -> Result<(), String> {
    validate_settings(&settings)?;
    if settings.executable.is_empty() || settings.work_dir.is_empty() {
        return Err("请先选择 OMP 可执行文件和工作目录".into());
    }
    let root = root_dir()?;
    let path = check_profile_path(&root, &settings.selected_profile)?;
    if settings.selected_profile != "default" && !path.is_dir() {
        return Err("该 profile 已不存在，请刷新后重新选择".into());
    }
    if broker_configured(&path.join("agent"))? {
        return Err("该 profile 配置了认证 broker，请新建独立 profile；不会覆盖现有配置".into());
    }
    check_launch_env_files(
        root.parent().ok_or("无法定位 OMP 用户目录")?,
        &path,
        Path::new(&settings.work_dir),
    )?;
    #[cfg(target_os = "macos")]
    {
        // The command is handed to Terminal, not retained as a Cockpit child.
        // Closing Cockpit therefore cannot terminate OMP or its refresh loop.
        let command = launch_command(&settings, login);
        let escaped = command
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        let script =
            format!("tell application \"Terminal\"\nactivate\ndo script \"{escaped}\"\nend tell");
        // Fail before spawning if preferences cannot be persisted: do not report
        // a failed launch after the terminal command has already been sent.
        save_settings(settings)?;
        let output = std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|_| "无法启动 macOS Terminal")?;
        if !output.status.success() {
            return Err("Terminal 启动失败，请检查系统自动化权限和终端状态".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = login;
        Err("OMP 原生终端启动暂仅支持 macOS".into())
    }
}

/// Login uses OMP's native local OAuth flow, never the broker server/gateway.
/// No work directory/profile chooser is needed for the default account store.
pub fn login(executable: String) -> Result<(), String> {
    let root = root_dir()?;
    let home = root.parent().ok_or("无法定位用户目录")?;
    launch(
        OmpSettings {
            executable,
            work_dir: home.to_string_lossy().into_owned(),
            selected_profile: "default".into(),
        },
        true,
    )
}

#[cfg(test)]
mod tests;
