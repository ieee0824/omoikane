use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub(super) tests: Vec<WptCase>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WptCase {
    pub(super) path: String,
    #[serde(default)]
    pub(super) known_failure: Option<KnownFailure>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct KnownFailure {
    pub(super) status: ActualStatus,
    pub(super) reason: String,
    pub(super) issue: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) expires: Option<String>,
    /// If present, only these FAIL subtests may account for the known failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) failed_subtests: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub(super) enum ActualStatus {
    Pass,
    Fail,
    Timeout,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Classification {
    Pass,
    KnownFailure,
    Regression,
    Improvement,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct WptReport {
    pub(super) revision: String,
    pub(super) summary: WptSummary,
    pub(super) results: Vec<WptResult>,
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(super) struct WptSummary {
    pub(super) total: usize,
    pub(super) pass: usize,
    pub(super) known_failure: usize,
    pub(super) regression: usize,
    pub(super) improvement: usize,
    pub(super) by_area: BTreeMap<String, AreaSummary>,
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(super) struct AreaSummary {
    pub(super) total: usize,
    pub(super) pass: usize,
    pub(super) known_failure: usize,
    pub(super) regression: usize,
    pub(super) improvement: usize,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct WptResult {
    pub(super) path: String,
    pub(super) area: String,
    pub(super) actual: ActualStatus,
    pub(super) classification: Classification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) known_failure: Option<KnownFailure>,
    pub(super) script_errors: Vec<String>,
    pub(super) subtests: serde_json::Value,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct WptAreaReport {
    pub(super) revision: String,
    pub(super) area: String,
    pub(super) summary: AreaSummary,
    pub(super) results: Vec<WptResult>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(super) struct WptRevisionDiff {
    pub(super) previous_revision: String,
    pub(super) current_revision: String,
    pub(super) known_failure_delta: i64,
    pub(super) regression_delta: i64,
    pub(super) improvement_delta: i64,
    pub(super) changed: Vec<WptResultChange>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(super) struct WptResultChange {
    pub(super) path: String,
    pub(super) previous: Option<Classification>,
    pub(super) current: Option<Classification>,
}
