//! Area totals and revision differences for WPT smoke reports.
use super::model::{
    Classification, WptReport, WptResult, WptResultChange, WptRevisionDiff, WptSummary,
};
use std::collections::BTreeMap;

pub(super) fn area_for_path(path: &str) -> String {
    path.split('/').next().unwrap_or("unknown").to_string()
}

pub(super) fn summarize(results: &[WptResult]) -> WptSummary {
    let mut summary = WptSummary {
        total: results.len(),
        ..WptSummary::default()
    };
    for result in results {
        let area = summary.by_area.entry(result.area.clone()).or_default();
        area.total += 1;
        match result.classification {
            Classification::Pass => {
                summary.pass += 1;
                area.pass += 1;
            }
            Classification::KnownFailure => {
                summary.known_failure += 1;
                area.known_failure += 1;
            }
            Classification::Regression => {
                summary.regression += 1;
                area.regression += 1;
            }
            Classification::Improvement => {
                summary.improvement += 1;
                area.improvement += 1;
            }
        }
    }
    summary
}

pub(super) fn diff_revision_reports(previous: &WptReport, current: &WptReport) -> WptRevisionDiff {
    let previous_results = previous
        .results
        .iter()
        .map(|result| (result.path.as_str(), result.classification))
        .collect::<BTreeMap<_, _>>();
    let current_results = current
        .results
        .iter()
        .map(|result| (result.path.as_str(), result.classification))
        .collect::<BTreeMap<_, _>>();
    let mut paths = previous_results
        .keys()
        .chain(current_results.keys())
        .copied()
        .collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();
    let changed = paths
        .into_iter()
        .filter_map(|path| {
            let previous = previous_results.get(path).copied();
            let current = current_results.get(path).copied();
            (previous != current).then(|| WptResultChange {
                path: path.to_string(),
                previous,
                current,
            })
        })
        .collect();
    WptRevisionDiff {
        previous_revision: previous.revision.clone(),
        current_revision: current.revision.clone(),
        known_failure_delta: current.summary.known_failure as i64
            - previous.summary.known_failure as i64,
        regression_delta: current.summary.regression as i64 - previous.summary.regression as i64,
        improvement_delta: current.summary.improvement as i64 - previous.summary.improvement as i64,
        changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ActualStatus;

    fn result(path: &str, classification: Classification) -> WptResult {
        WptResult {
            path: path.to_string(),
            area: area_for_path(path),
            actual: ActualStatus::Pass,
            classification,
            known_failure: None,
            script_errors: vec![],
            subtests: serde_json::json!([]),
        }
    }

    #[test]
    fn summary_counts_each_classification_and_orders_areas() {
        let results = vec![
            result("dom/z.html", Classification::Pass),
            result("css/b.html", Classification::KnownFailure),
            result("dom/a.html", Classification::Regression),
            result("css/a.html", Classification::Improvement),
        ];
        let original_order = results
            .iter()
            .map(|result| result.path.clone())
            .collect::<Vec<_>>();
        let summary = summarize(&results);

        assert_eq!(summary.total, 4);
        assert_eq!(summary.pass, 1);
        assert_eq!(summary.known_failure, 1);
        assert_eq!(summary.regression, 1);
        assert_eq!(summary.improvement, 1);
        assert_eq!(
            summary
                .by_area
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["css", "dom"]
        );
        let css = &summary.by_area["css"];
        assert_eq!(
            (
                css.total,
                css.pass,
                css.known_failure,
                css.regression,
                css.improvement
            ),
            (2, 0, 1, 0, 1)
        );
        let dom = &summary.by_area["dom"];
        assert_eq!(
            (
                dom.total,
                dom.pass,
                dom.known_failure,
                dom.regression,
                dom.improvement
            ),
            (2, 1, 0, 1, 0)
        );
        assert_eq!(
            results
                .iter()
                .map(|result| result.path.clone())
                .collect::<Vec<_>>(),
            original_order
        );
    }

    #[test]
    fn revision_diff_orders_added_changed_and_removed_paths() {
        let previous_results = vec![
            result("dom/z.html", Classification::Regression),
            result("dom/a.html", Classification::KnownFailure),
        ];
        let current_results = vec![
            result("dom/c.html", Classification::Improvement),
            result("dom/a.html", Classification::Pass),
        ];
        let previous = WptReport {
            revision: "old".to_string(),
            summary: summarize(&previous_results),
            results: previous_results,
        };
        let current = WptReport {
            revision: "new".to_string(),
            summary: summarize(&current_results),
            results: current_results,
        };

        let diff = diff_revision_reports(&previous, &current);
        assert_eq!(
            (
                diff.known_failure_delta,
                diff.regression_delta,
                diff.improvement_delta
            ),
            (-1, -1, 1)
        );
        assert_eq!(
            diff.changed
                .iter()
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>(),
            ["dom/a.html", "dom/c.html", "dom/z.html"]
        );
        assert_eq!(diff.changed[0].previous, Some(Classification::KnownFailure));
        assert_eq!(diff.changed[0].current, Some(Classification::Pass));
        assert_eq!(diff.changed[1].previous, None);
        assert_eq!(diff.changed[1].current, Some(Classification::Improvement));
        assert_eq!(diff.changed[2].previous, Some(Classification::Regression));
        assert_eq!(diff.changed[2].current, None);
    }

    #[test]
    fn revision_diff_reports_known_failure_changes() {
        let result = |classification| WptResult {
            path: "dom/a.html".to_string(),
            area: "dom".to_string(),
            actual: ActualStatus::Pass,
            classification,
            known_failure: None,
            script_errors: vec![],
            subtests: serde_json::json!([]),
        };
        let previous_results = vec![result(Classification::KnownFailure)];
        let current_results = vec![result(Classification::Pass)];
        let previous = WptReport {
            revision: "old".to_string(),
            summary: summarize(&previous_results),
            results: previous_results,
        };
        let current = WptReport {
            revision: "new".to_string(),
            summary: summarize(&current_results),
            results: current_results,
        };

        let diff = diff_revision_reports(&previous, &current);
        assert_eq!(diff.known_failure_delta, -1);
        assert_eq!(diff.regression_delta, 0);
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].previous, Some(Classification::KnownFailure));
        assert_eq!(diff.changed[0].current, Some(Classification::Pass));
    }
}
