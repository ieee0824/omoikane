//! Expected-outcome classification for WPT smoke cases.
use super::model::{ActualStatus, Classification, KnownFailure};

impl ActualStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Timeout => "TIMEOUT",
            Self::Error => "ERROR",
        }
    }
}

pub(super) fn classify(actual: ActualStatus, known: Option<&KnownFailure>) -> Classification {
    match (actual, known) {
        (ActualStatus::Pass, Some(_)) => Classification::Improvement,
        (ActualStatus::Pass, None) => Classification::Pass,
        (status, Some(known)) if status == known.status => Classification::KnownFailure,
        _ => Classification::Regression,
    }
}

pub(super) fn classify_with_subtests(
    actual: ActualStatus,
    known: Option<&KnownFailure>,
    subtests: &serde_json::Value,
) -> Classification {
    let classification = classify(actual, known);
    let Some(expected) = known.and_then(|known| known.failed_subtests.as_ref()) else {
        return classification;
    };
    if classification != Classification::KnownFailure {
        return classification;
    }
    let Some(subtests) = subtests.as_array() else {
        return Classification::Regression;
    };
    let mut failures = Vec::new();
    for subtest in subtests {
        match subtest["status"].as_u64() {
            Some(0) => {}
            Some(1) => {
                let Some(name) = subtest["name"].as_str() else {
                    return Classification::Regression;
                };
                failures.push(name);
            }
            _ => return Classification::Regression,
        }
    }
    failures.sort_unstable();
    let mut expected: Vec<_> = expected.iter().map(String::as_str).collect();
    expected.sort_unstable();
    if failures == expected {
        classification
    } else {
        Classification::Regression
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::known_failure;

    #[test]
    fn classifications_distinguish_known_failures_regressions_and_improvements() {
        assert_eq!(
            classify(ActualStatus::Fail, Some(&known_failure(ActualStatus::Fail))),
            Classification::KnownFailure
        );
        assert_eq!(
            classify(
                ActualStatus::Timeout,
                Some(&known_failure(ActualStatus::Timeout))
            ),
            Classification::KnownFailure
        );
        assert_eq!(
            classify(
                ActualStatus::Error,
                Some(&known_failure(ActualStatus::Timeout))
            ),
            Classification::Regression
        );
        assert_eq!(
            classify(ActualStatus::Fail, None),
            Classification::Regression
        );
        assert_eq!(
            classify(ActualStatus::Pass, Some(&known_failure(ActualStatus::Fail))),
            Classification::Improvement
        );
        assert_eq!(classify(ActualStatus::Pass, None), Classification::Pass);
    }

    #[test]
    fn exact_subtest_failures_do_not_hide_new_regressions() {
        use serde_json::json;
        let mut known = known_failure(ActualStatus::Fail);
        known.failed_subtests = Some(vec!["transfer".to_string()]);
        let classify = |results| classify_with_subtests(ActualStatus::Fail, Some(&known), &results);
        assert_eq!(
            classify(json!([{"name":"decode","status":0}, {"name":"transfer","status":1}])),
            Classification::KnownFailure
        );
        for results in [
            json!([{"name":"decode","status":1}, {"name":"transfer","status":1}]),
            json!([{"name":"transfer","status":2}]),
            json!([{"name":"transfer","status":0}]),
            json!([]),
            json!(null),
        ] {
            assert_eq!(classify(results), Classification::Regression);
        }
        assert_eq!(
            classify_with_subtests(ActualStatus::Pass, Some(&known), &json!([])),
            Classification::Improvement
        );
    }

    #[test]
    fn malformed_subtests_are_regressions_even_when_status_matches() {
        use serde_json::json;
        let mut known = known_failure(ActualStatus::Fail);
        known.failed_subtests = Some(vec!["transfer".to_string()]);
        for subtests in [
            json!([{"status": 1}]),
            json!([{"name": 7, "status": 1}]),
            json!([{"name": "transfer"}]),
            json!({"name": "transfer", "status": 1}),
        ] {
            assert_eq!(
                classify_with_subtests(ActualStatus::Fail, Some(&known), &subtests),
                Classification::Regression,
                "{subtests}"
            );
        }
    }
}
