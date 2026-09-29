use super::*;

fn package() -> CodexRegisteredLaunch {
    CodexRegisteredLaunch {
        family_name: "OpenAI.Codex_2p2nqsd0c76g0".into(),
        app_id: "CodexGui".into(),
        executable:
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.924_x64__2p2nqsd0c76g0\app\ChatGPT.exe"
                .into(),
    }
}

fn decode_inner_script(script: &str) -> String {
    use base64::{engine::general_purpose, Engine};
    let encoded = script
        .split("-EncodedCommand ")
        .nth(1)
        .unwrap()
        .split('\'')
        .next()
        .unwrap();
    let bytes = general_purpose::STANDARD.decode(encoded).unwrap();
    let units = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&units).unwrap()
}

#[test]
fn package_script_preserves_child_identity_and_hidden_helper() {
    let script =
        build_codex_package_identity_script(&package(), "home", Path::new("data"), &[], &[]);
    let activation = script
        .lines()
        .find(|line| line.starts_with("Invoke-CommandInDesktopPackage "))
        .unwrap();
    assert!(activation.contains("-PreventBreakaway"));
    assert!(activation.contains("-WindowStyle Hidden"));
    assert!(script.contains("$appId = 'CodexGui'"));
    assert!(script.contains("$app.Count -ne 1"));
    assert!(!script.contains("shell:AppsFolder"));
    assert!(decode_inner_script(&script).contains("$psi.UseShellExecute = $false"));
}

#[test]
fn arguments_have_exactly_one_selected_profile() {
    let args = vec![
        "--user-data-dir=old".into(),
        "--user-data-dir".into(),
        "old split path".into(),
        "--remote-debugging-port=9222".into(),
        "--proxy-server=http://localhost:8000".into(),
    ];
    let script = build_codex_package_identity_script(
        &package(),
        "home",
        Path::new(r"C:\Users\O'Brien\资料 space"),
        &args,
        &[],
    );
    let inner = decode_inner_script(&script);
    assert_eq!(inner.matches("--user-data-dir").count(), 1);
    assert!(!inner.contains("old split path"));
    assert!(inner.contains("--remote-debugging-port=9222"));
    assert!(inner.contains("--proxy-server=http://localhost:8000"));
    assert!(inner.contains("O''Brien"));
    assert!(inner.contains("资料 space"));
}

#[test]
fn preserves_injection_and_proxy_environment_without_overriding_isolation() {
    let script = build_codex_package_identity_script(
        &package(),
        "selected-home",
        Path::new("selected-data"),
        &[],
        &[
            (
                "NODE_OPTIONS".into(),
                "--require=\"C:\\a b\\hook.js\"".into(),
            ),
            ("HTTPS_PROXY".into(), "http://127.0.0.1:9123".into()),
            ("CODEX_HOME".into(), "wrong-home".into()),
        ],
    );
    let inner = decode_inner_script(&script);
    assert!(inner.contains("'NODE_OPTIONS', '--require=\"C:\\a b\\hook.js\"'"));
    assert!(inner.contains("'HTTPS_PROXY', 'http://127.0.0.1:9123'"));
    assert!(inner.contains("$env:CODEX_ELECTRON_USER_DATA_PATH = 'selected-data'"));
    assert!(
        inner.find("$env:CODEX_HOME = 'selected-home'").unwrap()
            > inner.find("wrong-home").unwrap()
    );
}

#[test]
fn registration_response_requires_gui_identity() {
    assert!(parse_codex_registered_launch("null").unwrap().is_none());
    let registered = parse_codex_registered_launch(r#"{"family_name":"OpenAI.Codex_publisher","app_id":"Gui","executable":"D:\\WindowsApps\\new\\app\\ChatGPT.exe"}"#).unwrap().unwrap();
    assert_eq!(registered.app_id, "Gui");
    for invalid in [
        "",
        "warning: null",
        r#"{"family_name":"","app_id":"App","executable":"ChatGPT.exe"}"#,
        r#"{"family_name":"pkg","app_id":"App","executable":"resources/codex.exe"}"#,
    ] {
        assert!(parse_codex_registered_launch(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn missing_or_other_profile_and_old_pids_cannot_confirm_launch() {
    let target = normalize_path_for_compare(r"C:\managed\profile");
    let entries = vec![
        (10, None),
        (11, Some(r"C:\default".into())),
        (12, Some(r"C:\managed\profile".into())),
    ];
    assert_eq!(
        codex_managed_launch_candidate(&entries, &target, &HashSet::from([12])),
        None
    );
    assert_eq!(
        codex_managed_launch_candidate(&entries, &target, &HashSet::new()),
        Some(12)
    );
    assert_eq!(
        codex_managed_launch_candidate(&[], &target, &HashSet::new()),
        None
    );
}

#[cfg(target_os = "windows")]
#[test]
fn registration_probe_refreshes_old_package_and_selects_gui_not_first_application() {
    // Execute the generated PowerShell with fake Appx registration. No app is launched.
    let setup = r#"
function Get-AppxPackage {
  [PSCustomObject]@{ Name='OpenAI.Codex'; Version=[version]'26.924'; PackageFamilyName='OpenAI.Codex_pub'; InstallLocation='C:\Program Files\WindowsApps\OpenAI.Codex_26.924_x64__pub' }
}
function Get-AppxPackageManifest {
  [PSCustomObject]@{ Package=@{ Applications=@{ Application=@(
    @{Id='Runner';Executable='app\resources\codex.exe'},
    @{Id='Gui';Executable='app\ChatGPT.exe'}
  ) } } }
}
function Test-Path { return $true }
"#;
    let script = format!(
        "{setup}\n{}",
        build_codex_registered_launch_probe(Path::new(
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.900_x64__pub\app\ChatGPT.exe"
        ))
    );
    let output = codex_launch_powershell_output(&script).unwrap();
    let registered = parse_codex_registered_launch(&output).unwrap().unwrap();
    assert_eq!(registered.app_id, "Gui");
    assert!(registered.executable.contains("26.924"));
    let script = format!(
        "{setup}\n{}",
        build_codex_registered_launch_probe(Path::new(r"C:\Tools\ChatGPT.exe"))
    );
    assert!(
        parse_codex_registered_launch(&codex_launch_powershell_output(&script).unwrap())
            .unwrap()
            .is_none()
    );
}

#[cfg(target_os = "windows")]
#[test]
fn package_identity_probe_rejects_unpackaged_process() {
    // cargo's test process is unpackaged. This exercises the real Windows API.
    assert!(verify_codex_process_package(std::process::id(), "OpenAI.Codex_pub").is_err());
}

#[test]
fn registered_store_route_never_attempts_direct_exe() {
    let package = package();
    let path = Path::new(&package.executable);
    assert_eq!(
        codex_managed_launch_route(path, Some(&package)).unwrap(),
        CodexManagedLaunchRoute::PackageIdentity
    );
    assert!(codex_managed_launch_route(path, None).is_err());
    // A registered package may live outside WindowsApps.
    assert_eq!(
        codex_managed_launch_route(Path::new(r"D:\Apps\Codex\ChatGPT.exe"), Some(&package))
            .unwrap(),
        CodexManagedLaunchRoute::PackageIdentity
    );
    assert_eq!(
        codex_managed_launch_route(Path::new(r"C:\Tools\ChatGPT.exe"), None).unwrap(),
        CodexManagedLaunchRoute::DirectExe
    );
}

#[test]
fn package_refresh_keeps_publisher_identity() {
    assert_eq!(
        codex_store_package_family_from_path(Path::new(&package().executable)).as_deref(),
        Some("openai.codex_2p2nqsd0c76g0")
    );
    assert_eq!(
        codex_store_package_family_from_path(Path::new(r"C:\Tools\ChatGPT.exe")),
        None
    );
    assert_eq!(
        codex_store_package_family_from_path(Path::new(
            r"C:\WindowsApps\OpenAI.Codex_1_x64__other\app\ChatGPT.exe"
        ))
        .as_deref(),
        Some("openai.codex_other")
    );
}

#[cfg(target_os = "windows")]
#[test]
fn activation_script_uses_matching_app_and_forces_child_identity_without_launching_gui() {
    // Exercise PowerShell parsing/binding while replacing only the system calls.
    let setup = r#"
function Get-AppxPackage {
  [PSCustomObject]@{ Version=[version]'26.924'; PackageFamilyName='OpenAI.Codex_2p2nqsd0c76g0'; InstallLocation='C:\Program Files\WindowsApps\OpenAI.Codex_26.924_x64__2p2nqsd0c76g0' }
}
function Get-AppxPackageManifest {
  [PSCustomObject]@{ Package=@{ Applications=@{ Application=@(
    @{Id='Runner';Executable='app\resources\codex.exe'},
    @{Id='CodexGui';Executable='app\ChatGPT.exe'}
  ) } } }
}
function Invoke-CommandInDesktopPackage {
  param($PackageFamilyName, $AppId, [switch]$PreventBreakaway, $Command, $Args)
  [PSCustomObject]@{ family=$PackageFamilyName; app=$AppId; inherited=[bool]$PreventBreakaway; command=$Command; arguments=$Args } | ConvertTo-Json -Compress
}
"#;
    let script = build_codex_package_identity_script(
        &package(),
        "managed-home",
        Path::new("managed-data"),
        &[],
        &[],
    );
    let output = codex_launch_powershell_output(&format!("{setup}\n{script}")).unwrap();
    let call: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(call["app"], "CodexGui");
    assert_eq!(call["family"], package().family_name);
    assert_eq!(call["inherited"], true);
    let arguments = call["arguments"].as_str().unwrap();
    assert!(arguments.contains("-WindowStyle Hidden"));
    assert!(arguments.contains("-EncodedCommand"));
    let inner = decode_inner_script(arguments);
    assert!(inner.contains("$env:CODEX_HOME = 'managed-home'"));

    let mut stale = package();
    stale.executable = stale.executable.replace("26.924", "26.900");
    let stale_script =
        build_codex_package_identity_script(&stale, "home", Path::new("data"), &[], &[]);
    assert!(codex_launch_powershell_output(&format!("{setup}\n{stale_script}")).is_err());
}
