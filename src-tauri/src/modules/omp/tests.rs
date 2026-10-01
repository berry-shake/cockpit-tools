use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cockpit-omp-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn profile_paths_reject_traversal_shell_payloads_and_reserved_names() {
    let root = Path::new("/tmp/omp-fixture");
    for name in [
        "",
        "../other",
        "a/b",
        "/absolute",
        "work;echo",
        "a\nother",
        ".",
        "..",
        "bad.",
        "con",
        "com1.txt",
        "UPPER",
    ] {
        assert!(profile_path(root, name).is_err(), "accepted {name:?}");
    }
    assert!(profile_path(root, &"a".repeat(65)).is_err());
    assert_eq!(profile_path(root, "default").unwrap(), root);
    assert_eq!(
        profile_path(root, "work-2.dev").unwrap(),
        root.join("profiles/work-2.dev")
    );
}

#[test]
fn creating_profile_never_copies_or_replaces_existing_credentials() {
    let fixture = Fixture::new();
    let root = fixture.0.join(".omp");
    fs::create_dir_all(root.join("agent")).unwrap();
    fs::write(
        root.join("agent/agent.db"),
        b"existing-default-credential-fixture",
    )
    .unwrap();
    create_profile_at(&root, "work").unwrap();
    assert!(root.join("profiles/work").is_dir());
    assert!(!root.join("profiles/work/agent/agent.db").exists());
    fs::write(root.join("profiles/work/keep.txt"), b"keep").unwrap();
    assert!(create_profile_at(&root, "work").is_err());
    assert!(create_profile_at(&root, "default").is_err());
    assert_eq!(
        fs::read(root.join("profiles/work/keep.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(
        fs::read(root.join("agent/agent.db")).unwrap(),
        b"existing-default-credential-fixture"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(root.join("profiles/work"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn state_scan_does_not_create_a_default_database_or_profile_directory() {
    let fixture = Fixture::new();
    let missing = fixture.0.join("not-created");
    let profiles = list_profiles(&missing).unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].name, "default");
    assert!(profiles[0].accounts.is_empty());
    assert!(!missing.exists());
}

#[test]
fn summaries_exclude_secrets_mcp_rows_and_do_not_mutate_the_database() {
    let fixture = Fixture::new();
    let agent = fixture.0.join("agent");
    fs::create_dir(&agent).unwrap();
    let path = agent.join("agent.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE auth_credentials (id INTEGER PRIMARY KEY, provider TEXT, credential_type TEXT, data TEXT, disabled_cause TEXT);").unwrap();
    let credential = serde_json::json!({"email":"test@example.invalid", "accountId":"workspace-1", "orgName":"Test org", "access":"access-secret-fixture", "refresh":"refresh-secret-fixture", "key":"api-secret-fixture"}).to_string();
    for (id, provider, disabled) in [
        (1, "openai-codex", None),
        (2, "anthropic", Some("revoked")),
        (3, "mcp_oauth:example", None),
    ] {
        db.execute(
            "INSERT INTO auth_credentials VALUES (?1, ?2, 'oauth', ?3, ?4)",
            rusqlite::params![id, provider, credential, disabled],
        )
        .unwrap();
    }
    drop(db);
    let before = fs::read(&path).unwrap();
    let summaries = read_account_summaries(&path).unwrap();
    assert_eq!(summaries.len(), 2);
    assert!(summaries[0].disabled);
    assert_eq!(summaries[1].email.as_deref(), Some("test@example.invalid"));
    assert!(!summaries[1].disabled);
    let serialized = serde_json::to_string(&summaries).unwrap();
    for secret in [
        "access-secret-fixture",
        "refresh-secret-fixture",
        "api-secret-fixture",
        "mcp_oauth:",
    ] {
        assert!(!serialized.contains(secret));
    }
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn one_broken_profile_does_not_hide_other_profiles_or_leak_database_content() {
    let fixture = Fixture::new();
    create_profile_at(&fixture.0, "broken").unwrap();
    create_profile_at(&fixture.0, "healthy").unwrap();
    fs::create_dir(fixture.0.join("profiles/broken/agent")).unwrap();
    fs::write(
        fixture.0.join("profiles/broken/agent/agent.db"),
        b"credential-secret-invalid-db",
    )
    .unwrap();
    let profiles = list_profiles(&fixture.0).unwrap();
    assert_eq!(
        profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["default", "broken", "healthy"]
    );
    assert!(profiles[1].warning.is_some());
    assert!(profiles[2].warning.is_none());
    assert!(!serde_json::to_string(&profiles)
        .unwrap()
        .contains("credential-secret"));
}

#[test]
fn broker_profiles_are_not_offered_as_independent_and_configs_are_preserved() {
    let fixture = Fixture::new();
    let agent = fixture.0.join("agent");
    fs::create_dir(&agent).unwrap();
    let path = agent.join("config.yml");
    for text in [
        "auth:\n  broker:\n    url: http://localhost:8765\n",
        "auth.broker.url: http://localhost:8765\n",
    ] {
        fs::write(&path, text).unwrap();
        let profile = describe_profile(&fixture.0, "default");
        assert!(!profile.independent);
        assert!(profile.warning.unwrap().contains("broker"));
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
    fs::write(&path, "# empty config\n").unwrap();
    // The canonical .yml wins even when the fallback .yaml contains a broker.
    fs::write(
        agent.join("config.yaml"),
        "auth.broker.url: http://localhost:8765",
    )
    .unwrap();
    assert!(!broker_configured(&agent).unwrap());
    fs::write(&path, "auth: [invalid: yaml: secret").unwrap();
    let profile = describe_profile(&fixture.0, "default");
    assert!(!profile.independent);
    assert!(!profile.warning.unwrap().contains("secret"));
}

#[test]
fn dotenv_cannot_silently_restore_broker_or_redirect_profile_storage() {
    let fixture = Fixture::new();
    let file = fixture.0.join(".env");
    fs::write(&file, "# OMP_AUTH_BROKER_URL=comment\nNORMAL=value\n").unwrap();
    check_dotenv_overrides(&file).unwrap();
    for key in [
        "OMP_AUTH_BROKER_URL",
        "OMP_CONFIG_DIR",
        "PI_CODING_AGENT_DIR",
        "XDG_DATA_HOME",
    ] {
        let text = format!("export {key}=secret-fixture\n");
        fs::write(&file, &text).unwrap();
        let error = check_dotenv_overrides(&file).unwrap_err();
        assert!(!error.contains("secret-fixture"));
        assert_eq!(fs::read_to_string(&file).unwrap(), text);
    }
    let home = fixture.0.join("home");
    let profile = fixture.0.join("profile");
    let work = fixture.0.join("work");
    fs::create_dir(&work).unwrap();
    fs::write(
        work.join(".env.production.local"),
        "PI_PROFILE=wrong-profile",
    )
    .unwrap();
    assert!(check_launch_env_files(&home, &profile, &work).is_err());
}

#[cfg(unix)]
#[test]
fn symlinked_profiles_and_databases_are_never_followed() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let root = fixture.0.join(".omp");
    let outside = fixture.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::create_dir_all(root.join("profiles")).unwrap();
    symlink(&outside, root.join("profiles/linked")).unwrap();
    assert!(create_profile_at(&root, "linked").is_err());
    assert!(!describe_profile(&root, "linked").independent);
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    fs::write(outside.join("secret.db"), b"secret").unwrap();
    symlink(outside.join("secret.db"), root.join("alias.db")).unwrap();
    assert!(read_account_summaries(&root.join("alias.db")).is_err());
}

#[cfg(unix)]
#[test]
fn launch_preserves_literal_arguments_and_clears_inherited_profile_and_broker() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let work = fixture.0.join("work ' $() ; space");
    fs::create_dir(&work).unwrap();
    let executable = fixture.0.join("omp ' $(not-a-command)");
    fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"${OMP_AUTH_BROKER_URL-unset}\" \"${PI_CONFIG_DIR-unset}\" \"${PI_CODING_AGENT_DIR-unset}\" \"${XDG_DATA_HOME-unset}\" \"$@\"\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let settings = OmpSettings {
        executable: executable.to_string_lossy().into(),
        work_dir: work.to_string_lossy().into(),
        selected_profile: "work".into(),
    };
    validate_settings(&settings).unwrap();
    for login in [false, true] {
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(launch_command(&settings, login))
            .env("OMP_AUTH_BROKER_URL", "http://fixture.invalid")
            .env("PI_CONFIG_DIR", "wrong-root")
            .env("PI_CODING_AGENT_DIR", "/wrong/agent")
            .env("XDG_DATA_HOME", "/wrong/data")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let lines: Vec<_> = stdout.lines().collect();
        assert_eq!(lines[0], settings.work_dir);
        assert_eq!(&lines[1..5], ["unset", "unset", "unset", "unset"]);
        assert_eq!(&lines[5..7], ["--profile", "work"]);
        if login {
            assert_eq!(&lines[7..], ["auth-broker", "login"]);
        } else {
            assert_eq!(&lines[7..], ["--cwd", settings.work_dir.as_str()]);
        }
    }
}
