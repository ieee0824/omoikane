use super::model::{ActualStatus, Manifest};
use std::collections::HashSet;

pub(super) fn validate_manifest(manifest: &Manifest) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let mut paths = HashSet::new();
    for case in &manifest.tests {
        if case.path.trim().is_empty() {
            errors.push("test path must not be empty".to_string());
        } else if !paths.insert(case.path.as_str()) {
            errors.push(format!("duplicate test path: {}", case.path));
        }
        if let Some(known) = &case.known_failure {
            if let Some(names) = &known.failed_subtests {
                let unique: HashSet<_> = names.iter().collect();
                if known.status != ActualStatus::Fail
                    || names.is_empty()
                    || names.iter().any(|name| name.trim().is_empty())
                    || unique.len() != names.len()
                {
                    errors.push(format!(
                        "{}: failed_subtests requires FAIL and nonempty unique names",
                        case.path
                    ));
                }
            }
            if known.status == ActualStatus::Pass {
                errors.push(format!(
                    "{}: known failure status must not be PASS",
                    case.path
                ));
            }
            if known.reason.trim().is_empty() {
                errors.push(format!(
                    "{}: known failure reason must not be empty",
                    case.path
                ));
            }
            if known.issue.trim().is_empty() {
                errors.push(format!(
                    "{}: known failure issue must not be empty",
                    case.path
                ));
            }
            if known
                .expires
                .as_ref()
                .is_some_and(|value| value.trim().is_empty())
            {
                errors.push(format!(
                    "{}: known failure expires must not be empty",
                    case.path
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[test]
fn manifest_validation_rejects_duplicates_and_invalid_known_failures() {
    let manifest: Manifest = serde_json::from_value(serde_json::json!({"tests": [
        {"path": "dom/a.html"},
        {"path": "dom/a.html"},
        {"path": "  "},
        {"path": "dom/b.html", "known_failure": {
            "status": "PASS", "reason": "", "issue": "", "expires": ""
        }}
    ]}))
    .unwrap();
    let errors = validate_manifest(&manifest).unwrap_err().join("\n");
    assert_eq!(
        errors,
        concat!(
            "duplicate test path: dom/a.html\n",
            "test path must not be empty\n",
            "dom/b.html: known failure status must not be PASS\n",
            "dom/b.html: known failure reason must not be empty\n",
            "dom/b.html: known failure issue must not be empty\n",
            "dom/b.html: known failure expires must not be empty"
        )
    );
    assert!(
        serde_json::from_value::<Manifest>(serde_json::json!({
            "tests": [{"path": "dom/a.html", "expected": "PASS"}]
        }))
        .is_err(),
        "legacy expected metadata must be rejected"
    );
}

#[test]
fn manifest_validation_rejects_invalid_failed_subtests() {
    use serde_json::json;
    for (status, names) in [
        ("FAIL", json!([])),
        ("FAIL", json!([""])),
        ("FAIL", json!(["a", "a"])),
        ("TIMEOUT", json!(["a"])),
    ] {
        let manifest: Manifest = serde_json::from_value(json!({"tests":[{
            "path":"encoding/a.any.js", "known_failure": {
                "status":status, "reason":"reason", "issue":"#763", "failed_subtests": names
            }
        }]}))
        .unwrap();
        assert_eq!(
            validate_manifest(&manifest).unwrap_err(),
            vec!["encoding/a.any.js: failed_subtests requires FAIL and nonempty unique names"]
        );
    }
}

#[test]
fn manifest_validation_accepts_valid_known_failure() {
    let manifest: Manifest = serde_json::from_value(serde_json::json!({"tests": [
        {"path": "dom/ok.html"},
        {"path": "dom/fail.html", "known_failure": {
            "status": "FAIL",
            "reason": "tracked",
            "issue": "#889",
            "expires": "2026-12-31",
            "failed_subtests": ["first", "second"]
        }}
    ]}))
    .unwrap();
    assert!(validate_manifest(&manifest).is_ok());
}
