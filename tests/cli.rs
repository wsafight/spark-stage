use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("skills/screenwriter/examples")
        .join(name)
}

#[test]
fn valid_bundle_returns_success_and_machine_summary() {
    let output = Command::new(env!("CARGO_BIN_EXE_sparkstage"))
        .args(["script", "validate"])
        .arg(fixture("valid-short-drama.json"))
        .arg("--json")
        .output()
        .unwrap();

    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["valid"], true);
    assert_eq!(report["code"], "SCRIPT_BUNDLE_VALID");
    assert_eq!(report["summary"]["shots"], 2);
}

#[test]
fn invalid_bundle_returns_two_and_json_pointer() {
    let output = Command::new(env!("CARGO_BIN_EXE_sparkstage"))
        .args(["script", "validate"])
        .arg(fixture("invalid-workflow-field.json"))
        .arg("--json")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["valid"], false);
    assert_eq!(report["code"], "SCRIPT_BUNDLE_INVALID");
    assert_eq!(report["errors"][0]["path"], "/shots/0/steps");
}

#[test]
fn disabled_example_adapter_is_reported_without_network_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_sparkstage"))
        .arg("preflight")
        .arg("--adapter-config")
        .arg(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("adapters/minimax-h3-comfy.example.yaml"),
        )
        .args(["--minimum-free-gib", "0", "--json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["ready"], false);
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["code"] == "ADAPTER_DISABLED")
    );
    assert_eq!(
        report["adapters"][0]["report"]["operations"]["t2v"]["status"],
        "unavailable"
    );
}
