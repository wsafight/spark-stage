use serde::Deserialize;

use super::*;

const VALID: &str = include_str!("../../skills/screenwriter/examples/valid-short-drama.json");

#[test]
fn accepts_valid_bundle() {
    let result = validate_json(VALID);
    assert!(result.is_valid(), "issues: {:#?}", result.issues);
}

#[test]
fn checked_in_schema_matches_rust_types() {
    let checked_in: serde_json::Value =
        serde_json::from_str(include_str!("../../schemas/script-bundle.schema.json")).unwrap();
    assert_eq!(checked_in, json_schema());
}

#[test]
fn reports_unknown_speaker_with_json_pointer() {
    let mut value: serde_json::Value = serde_json::from_str(VALID).unwrap();
    value["shots"][0]["dialogue"][0]["who"] = serde_json::json!("unknown");

    let result = validate_json(&serde_json::to_string(&value).unwrap());
    assert!(result.issues.iter().any(|issue| {
        issue.code == "CHARACTER_SCOPE" && issue.path == "/shots/0/dialogue/0/who"
    }));
}

#[test]
fn rejects_backend_specific_unknown_field() {
    let mut value: serde_json::Value = serde_json::from_str(VALID).unwrap();
    value["shots"][0]["steps"] = serde_json::json!(12);

    let result = validate_json(&serde_json::to_string(&value).unwrap());
    assert!(!result.is_valid());
    assert_eq!(result.issues[0].code, "JSON_CONTRACT");
    assert!(result.issues[0].message.contains("unknown field `steps`"));
}

#[test]
fn rejects_dialogue_that_exceeds_shot_budget() {
    let mut value: serde_json::Value = serde_json::from_str(VALID).unwrap();
    value["shots"][0]["dialogue"][0]["text"] = serde_json::json!(
        "这句话被故意写得非常非常长，因为五秒钟的镜头不可能在保留呼吸空间的同时完整说完。"
    );

    let result = validate_json(&serde_json::to_string(&value).unwrap());
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.code == "DIALOGUE_BUDGET")
    );
}

#[test]
fn rejects_conditioning_that_does_not_match_operation() {
    let mut value: serde_json::Value = serde_json::from_str(VALID).unwrap();
    value["shots"][0]["operation"] = serde_json::json!("flf2v");
    value["shots"][0]["conditioning"] = serde_json::json!({
        "first_frame": "refs/shots/S01-start.png",
        "reference_images": []
    });

    let result = validate_json(&serde_json::to_string(&value).unwrap());
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.code == "CONDITIONING_MISMATCH")
    );
}

#[test]
fn rejects_continuity_state_mismatch() {
    let mut value: serde_json::Value = serde_json::from_str(VALID).unwrap();
    value["shots"][1]["continuity"]["state_in"]["door"] = serde_json::json!("open");

    let result = validate_json(&serde_json::to_string(&value).unwrap());
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.code == "CONTINUITY_STATE_MISMATCH")
    );
}

#[test]
fn rejects_colliding_audition_and_final_profiles() {
    let mut value: serde_json::Value = serde_json::from_str(VALID).unwrap();
    value["shots"][0]["generation_plan"]["final_profile"] =
        value["shots"][0]["generation_plan"]["audition_profile"].clone();

    let result = validate_json(&serde_json::to_string(&value).unwrap());

    assert!(result.issues.iter().any(|issue| {
        issue.code == "PROFILE_COLLISION" && issue.path == "/shots/0/generation_plan/final_profile"
    }));
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentFixtureSuite {
    schema_version: String,
    cases: Vec<AgentFixtureExpectation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentFixtureExpectation {
    bundle: String,
    valid: bool,
    project_id: Option<String>,
    shots: Option<usize>,
    duration_seconds: Option<u32>,
    agent_host: Option<String>,
    issue_codes: Vec<String>,
}

#[test]
fn external_agent_script_bundles_match_checked_in_expectations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let suite: AgentFixtureSuite = serde_json::from_slice(
        &std::fs::read(root.join("tests/fixtures/agent-script-bundles/expectations.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(suite.schema_version, SUPPORTED_SCHEMA_VERSION);
    assert!(suite.cases.len() >= 3);

    for expected in suite.cases {
        let source = std::fs::read_to_string(root.join(&expected.bundle)).unwrap();
        let result = validate_json(&source);
        assert_eq!(
            result.is_valid(),
            expected.valid,
            "{}: {:#?}",
            expected.bundle,
            result.issues
        );
        assert_eq!(
            result
                .issues
                .iter()
                .map(|issue| issue.code.to_owned())
                .collect::<Vec<_>>(),
            expected.issue_codes,
            "{}",
            expected.bundle
        );
        if let Some(bundle) = result.bundle {
            assert_eq!(
                Some(bundle.project.id),
                expected.project_id,
                "{}",
                expected.bundle
            );
            assert_eq!(
                Some(bundle.shots.len()),
                expected.shots,
                "{}",
                expected.bundle
            );
            assert_eq!(
                Some(bundle.shots.iter().map(|shot| shot.duration).sum()),
                expected.duration_seconds,
                "{}",
                expected.bundle
            );
            assert_eq!(
                bundle.authoring.and_then(|authoring| authoring.agent_host),
                expected.agent_host,
                "{}",
                expected.bundle
            );
        } else {
            assert!(expected.project_id.is_none());
            assert!(expected.shots.is_none());
            assert!(expected.duration_seconds.is_none());
            assert!(expected.agent_host.is_none());
        }
    }
}
