// Provider snapshot regression tests use only temporary profiles and fake URLs.
fn snapshot_test_provider() -> super::ApiProviderConfig {
    resolve_api_provider_config(
        Some("https://relay.example.test/v1"),
        Some(CodexApiProviderMode::Custom),
        Some("relay"),
        Some("Relay"),
    ).unwrap()
}

fn snapshot_test_builtin() -> super::ApiProviderConfig {
    resolve_api_provider_config(
        None, Some(CodexApiProviderMode::OpenaiBuiltin), None, None,
    ).unwrap()
}

#[test]
fn provider_snapshot_roundtrip_preserves_first_original_and_cleans_up_after_commit() {
    let base_dir = make_temp_dir("provider-snapshot-roundtrip");
    let config_path = base_dir.join("config.toml");
    let original = "model = \"original-model\"\n[model_providers.relay]\nname = \"Original\"\nbase_url = \"https://original.example.test/v1\"\n";
    fs::write(&config_path, original).unwrap();
    super::write_api_provider_to_config_toml_with_options(&base_dir, &snapshot_test_provider(), false).unwrap();
    let snapshot_path = super::provider_override_snapshot_path(&base_dir);
    let first_snapshot = fs::read(&snapshot_path).unwrap();
    super::write_api_provider_to_config_toml_with_options(&base_dir, &snapshot_test_provider(), false).unwrap();
    assert_eq!(fs::read(&snapshot_path).unwrap(), first_snapshot);
    super::write_api_provider_to_config_toml_with_options(&base_dir, &snapshot_test_builtin(), false).unwrap();
    let restored = fs::read_to_string(&config_path).unwrap().parse::<toml_edit::Document>().unwrap();
    assert_eq!(restored["model"].as_str(), Some("original-model"));
    assert!(restored.get("model_provider").is_none());
    assert_eq!(restored["model_providers"]["relay"]["name"].as_str(), Some("Original"));
    assert_eq!(restored["model_providers"]["relay"]["base_url"].as_str(), Some("https://original.example.test/v1"));
    assert!(!snapshot_path.exists());
    fs::remove_dir_all(base_dir).unwrap();
}

#[test]
fn provider_snapshot_corruption_blocks_takeover_and_restore_without_deleting_backup() {
    let base_dir = make_temp_dir("provider-snapshot-corrupt");
    let config_path = base_dir.join("config.toml");
    let original = "model = \"keep-model\"\nmodel_provider = \"relay\"\n";
    fs::write(&config_path, original).unwrap();
    let snapshot_path = super::provider_override_snapshot_path(&base_dir);
    for content in [
        "{broken-json",
        r#"{"providerId":"relay","providerTable":"invalid = ["}"#,
        r#"{"providerId":""}"#,
    ] {
        fs::write(&snapshot_path, content).unwrap();
        for provider in [snapshot_test_provider(), snapshot_test_builtin()] {
            assert!(super::write_api_provider_to_config_toml_with_options(&base_dir, &provider, false).is_err());
            assert_eq!(fs::read_to_string(&config_path).unwrap(), original);
            assert_eq!(fs::read_to_string(&snapshot_path).unwrap(), content);
        }
    }
    fs::remove_dir_all(base_dir).unwrap();
}

#[test]
fn provider_snapshot_unreadable_path_blocks_takeover() {
    let base_dir = make_temp_dir("provider-snapshot-unreadable");
    let config_path = base_dir.join("config.toml");
    let original = "model = \"keep-model\"\n";
    fs::write(&config_path, original).unwrap();
    let snapshot_path = super::provider_override_snapshot_path(&base_dir);
    fs::create_dir(&snapshot_path).unwrap();
    assert!(super::write_api_provider_to_config_toml_with_options(&base_dir, &snapshot_test_provider(), false).is_err());
    assert_eq!(fs::read_to_string(&config_path).unwrap(), original);
    assert!(snapshot_path.is_dir());
    fs::remove_dir_all(base_dir).unwrap();
}

#[test]
fn provider_snapshot_config_read_error_is_not_treated_as_empty_profile() {
    let base_dir = make_temp_dir("provider-config-unreadable");
    let config_path = base_dir.join("config.toml");
    fs::create_dir(&config_path).unwrap();
    assert!(super::write_api_provider_to_config_toml_with_options(&base_dir, &snapshot_test_provider(), false).is_err());
    assert!(config_path.is_dir());
    assert!(!super::provider_override_snapshot_path(&base_dir).exists());
    fs::remove_dir_all(base_dir).unwrap();
}

#[test]
fn provider_snapshot_config_write_failure_keeps_recoverable_original() {
    let base_dir = make_temp_dir("provider-snapshot-write-failure");
    let mut doc = "model = \"original-model\"\n".parse::<toml_edit::Document>().unwrap();
    super::record_provider_override_snapshot(&doc, &base_dir, "relay").unwrap();
    let snapshot_path = super::provider_override_snapshot_path(&base_dir);
    let snapshot = fs::read(&snapshot_path).unwrap();
    doc["model_provider"] = toml_edit::value("relay");
    doc["model"] = toml_edit::value("managed-model");
    let hash = super::restore_provider_override_snapshot(&base_dir, &mut doc).unwrap();
    assert_eq!(doc["model"].as_str(), Some("original-model"));
    assert_eq!(fs::read(&snapshot_path).unwrap(), snapshot);
    let error = super::persist_provider_config_with_snapshot_cleanup(
        &base_dir, &doc.to_string(), hash, |_, _| Err("injected disk write failure".into()),
    ).unwrap_err();
    assert!(error.contains("injected disk write failure"));
    assert_eq!(fs::read(&snapshot_path).unwrap(), snapshot);
    super::persist_provider_config_with_snapshot_cleanup(
        &base_dir, &doc.to_string(), hash,
        crate::modules::codex_config_format::write_codex_config_toml_atomic,
    ).unwrap();
    assert!(!snapshot_path.exists());
    fs::remove_dir_all(base_dir).unwrap();
}

#[test]
fn provider_snapshot_cleanup_does_not_delete_a_replaced_snapshot() {
    let base_dir = make_temp_dir("provider-snapshot-replaced");
    let mut doc = toml_edit::Document::new();
    super::record_provider_override_snapshot(&doc, &base_dir, "relay").unwrap();
    let snapshot_path = super::provider_override_snapshot_path(&base_dir);
    doc["model_provider"] = toml_edit::value("relay");
    let hash = super::restore_provider_override_snapshot(&base_dir, &mut doc).unwrap();
    let replacement = r#"{"providerId":"another-provider"}"#;
    super::persist_provider_config_with_snapshot_cleanup(
        &base_dir, &doc.to_string(), hash, |path, content| {
            crate::modules::codex_config_format::write_codex_config_toml_atomic(path, content)?;
            fs::write(&snapshot_path, replacement).map_err(|error| error.to_string())
        },
    ).unwrap();
    assert_eq!(fs::read_to_string(&snapshot_path).unwrap(), replacement);
    fs::remove_dir_all(base_dir).unwrap();
}
