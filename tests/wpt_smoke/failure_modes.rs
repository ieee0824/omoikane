//! End-to-end checks for the smoke runner's failure contract.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    wpt_root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "omoikane-wpt-smoke-failure-modes-{}-{unique}",
            std::process::id()
        ));
        let wpt_root = root.join("wpt");
        fs::create_dir_all(root.join("tests/wpt")).unwrap();
        fs::create_dir_all(wpt_root.join("resources")).unwrap();
        fs::create_dir_all(wpt_root.join("dom")).unwrap();
        fs::write(root.join("tests/wpt/revision.txt"), b"synthetic\n").unwrap();
        fs::write(
            wpt_root.join("resources/testharness.js"),
            b"function setup() {}\nfunction add_result_callback(callback) { globalThis.__emit_result = callback; }\nfunction add_completion_callback(callback) { globalThis.__emit_complete = callback; }\n",
        )
        .unwrap();
        let fixture = Self { root, wpt_root };
        fixture.set_manifest(None);
        fixture.set_case_status(0);
        fixture
    }

    fn set_manifest(&self, known_status: Option<&str>) {
        let case = if let Some(status) = known_status {
            serde_json::json!({
                "path": "dom/synthetic.any.js",
                "known_failure": {
                    "status": status,
                    "reason": "synthetic expected failure",
                    "issue": "#895"
                }
            })
        } else {
            serde_json::json!({"path": "dom/synthetic.any.js"})
        };
        fs::write(
            self.root.join("tests/wpt/manifest.json"),
            serde_json::to_vec(&serde_json::json!({"tests": [case]})).unwrap(),
        )
        .unwrap();
    }

    fn set_case_status(&self, status: u8) {
        fs::write(
            self.wpt_root.join("dom/synthetic.any.js"),
            format!(
                "__emit_result({{name:'synthetic',status:{status},message:''}});\n__emit_complete([], {{status:0}});\n"
            ),
        )
        .unwrap();
    }

    fn set_case_source(&self, source: &str) {
        fs::write(self.wpt_root.join("dom/synthetic.any.js"), source).unwrap();
    }

    fn run(&self, wpt_root: &Path, required: bool, report: Option<&Path>) -> Output {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg("selected_wpt_testharness_cases_match_expectations")
            .arg("--nocapture")
            .current_dir(&self.root)
            .env("WPT_ROOT", wpt_root)
            .env_remove("WPT_REQUIRED")
            .env_remove("WPT_REPORT")
            .env_remove("WPT_JUNIT")
            .env_remove("WPT_RESULTS_DIR")
            .env_remove("WPT_COMPARE_REVISION")
            .env_remove("GITHUB_ACTIONS");
        if required {
            command.env("WPT_REQUIRED", "1");
        }
        if let Some(report) = report {
            command.env("WPT_REPORT", report);
        }
        command.output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn missing_checkout_is_optional_unless_required() {
    let fixture = Fixture::new();
    let missing = fixture.root.join("missing-wpt");
    let optional = fixture.run(&missing, false, None);
    assert!(optional.status.success(), "{}", output_text(&optional));
    assert!(output_text(&optional).contains("WPT checkout missing"));

    let required = fixture.run(&missing, true, None);
    assert!(!required.status.success(), "{}", output_text(&required));
    assert!(output_text(&required).contains("WPT checkout required but missing"));
}

#[test]
fn report_write_failure_fails_the_runner() {
    let fixture = Fixture::new();
    let file = fixture.root.join("not-a-directory");
    fs::write(&file, b"file").unwrap();
    let output = fixture.run(&fixture.wpt_root, true, Some(&file.join("report.json")));
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(output_text(&output).contains("create report directory"));
}

#[test]
fn regression_and_unexpected_pass_fail_the_runner() {
    let fixture = Fixture::new();
    fixture.set_case_status(1);
    let regression = fixture.run(&fixture.wpt_root, true, None);
    assert!(!regression.status.success(), "{}", output_text(&regression));
    assert!(output_text(&regression).contains("WPT dom/synthetic.any.js regression"));

    fixture.set_case_status(0);
    fixture.set_manifest(Some("FAIL"));
    let improvement = fixture.run(&fixture.wpt_root, true, None);
    assert!(
        !improvement.status.success(),
        "{}",
        output_text(&improvement)
    );
    assert!(output_text(&improvement).contains("WPT dom/synthetic.any.js unexpected pass"));
}

#[test]
fn raw_timeout_and_script_error_are_preserved_in_reports() {
    let fixture = Fixture::new();
    let report_path = fixture.root.join("report.json");

    fixture.set_manifest(Some("TIMEOUT"));
    fixture.set_case_source("// intentionally never complete the harness\n");
    let timeout = fixture.run(&fixture.wpt_root, true, Some(&report_path));
    assert!(timeout.status.success(), "{}", output_text(&timeout));
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["results"][0]["actual"], "TIMEOUT");
    assert_eq!(report["results"][0]["classification"], "known-failure");

    fixture.set_manifest(Some("ERROR"));
    fixture.set_case_source("throw new Error('synthetic script error');\n");
    let error = fixture.run(&fixture.wpt_root, true, Some(&report_path));
    assert!(error.status.success(), "{}", output_text(&error));
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["results"][0]["actual"], "ERROR");
    assert_eq!(report["results"][0]["classification"], "known-failure");
    assert!(
        !report["results"][0]["script_errors"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
