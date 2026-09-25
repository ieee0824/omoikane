//! JSON, JUnit and revision-scoped report serialization.
use super::model::{Classification, WptAreaReport, WptReport};
use std::fs;
use std::path::Path;

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

pub(super) fn junit_xml(report: &WptReport) -> String {
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"wpt-smoke\" tests=\"{}\" failures=\"{}\">\n",
        report.results.len(),
        report.summary.regression
    );
    for result in &report.results {
        let details = serde_json::to_string(&result.subtests).expect("serialize WPT subtests");
        let escaped_details = escape_xml(&details);
        let status = match result.classification {
            Classification::KnownFailure => " known-failure=\"true\"",
            Classification::Improvement => " improvement=\"true\"",
            _ => "",
        };
        xml.push_str(&format!(
            "  <testcase classname=\"wpt.{}\" name=\"{}\"{}>\n",
            escape_xml(&result.area),
            escape_xml(&result.path),
            status
        ));
        match result.classification {
            Classification::Regression => {
                let expected = result
                    .known_failure
                    .as_ref()
                    .map(|known| known.status.as_str())
                    .unwrap_or("PASS");
                xml.push_str(&format!(
                    "    <failure message=\"expected {}, got {}\">{}</failure>\n",
                    escape_xml(expected),
                    result.actual.as_str(),
                    escaped_details
                ));
            }
            Classification::KnownFailure => {
                let known = result
                    .known_failure
                    .as_ref()
                    .expect("known failure metadata");
                xml.push_str(&format!(
                    "    <system-out>KNOWN FAILURE [{}] {}: {}\n{}</system-out>\n",
                    result.actual.as_str(),
                    escape_xml(&known.issue),
                    escape_xml(&known.reason),
                    escaped_details
                ));
            }
            Classification::Improvement => {
                let known = result.known_failure.as_ref().expect("improvement metadata");
                xml.push_str(&format!(
                    "    <system-out>IMPROVEMENT: passed despite known {} failure ({})\n{}</system-out>\n",
                    known.status.as_str(), escape_xml(&known.issue), escaped_details
                ));
            }
            Classification::Pass => {}
        }
        if matches!(
            result.classification,
            Classification::Pass | Classification::Regression
        ) {
            xml.push_str(&format!(
                "    <system-out>{}</system-out>\n",
                escaped_details
            ));
        }
        xml.push_str("  </testcase>\n");
    }
    xml.push_str("</testsuite>\n");
    xml
}

pub(super) fn write_revision_reports(root: &Path, report: &WptReport) -> std::io::Result<()> {
    let revision_dir = root.join(&report.revision);
    fs::create_dir_all(&revision_dir)?;
    fs::write(
        revision_dir.join("report.json"),
        serde_json::to_vec_pretty(report).map_err(std::io::Error::other)?,
    )?;
    for (area, summary) in &report.summary.by_area {
        let area_report = WptAreaReport {
            revision: report.revision.clone(),
            area: area.clone(),
            summary: summary.clone(),
            results: report
                .results
                .iter()
                .filter(|result| result.area == *area)
                .cloned()
                .collect(),
        };
        let filename = area
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        fs::write(
            revision_dir.join(format!("{filename}.json")),
            serde_json::to_vec_pretty(&area_report).map_err(std::io::Error::other)?,
        )?;
    }
    Ok(())
}

pub(super) fn read_revision_report(root: &Path, revision: &str) -> std::io::Result<WptReport> {
    let bytes = fs::read(root.join(revision).join("report.json"))?;
    serde_json::from_slice(&bytes).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ActualStatus, WptResult, known_failure};
    use crate::summary::summarize;

    #[test]
    fn revision_reports_round_trip_and_split_by_area() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "omoikane-wpt-results-{}-{unique}",
            std::process::id(),
        ));
        let results = vec![
            WptResult {
                path: "dom/a.html".to_string(),
                area: "dom".to_string(),
                actual: ActualStatus::Pass,
                classification: Classification::Pass,
                known_failure: None,
                script_errors: vec![],
                subtests: serde_json::json!([]),
            },
            WptResult {
                path: "css/b.html".to_string(),
                area: "css".to_string(),
                actual: ActualStatus::Timeout,
                classification: Classification::KnownFailure,
                known_failure: Some(known_failure(ActualStatus::Timeout)),
                script_errors: vec![],
                subtests: serde_json::json!([]),
            },
        ];
        let report = WptReport {
            revision: "abc123".to_string(),
            summary: summarize(&results),
            results,
        };

        write_revision_reports(&root, &report).unwrap();
        assert_eq!(read_revision_report(&root, "abc123").unwrap(), report);
        let area: WptAreaReport =
            serde_json::from_slice(&fs::read(root.join("abc123/css.json")).unwrap()).unwrap();
        assert_eq!(area.area, "css");
        assert_eq!(area.summary.known_failure, 1);
        assert_eq!(area.results.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn junit_report_escapes_xml_and_reports_mismatches() {
        let results = vec![WptResult {
            path: "a<&\"'".to_string(),
            area: "dom".to_string(),
            actual: ActualStatus::Fail,
            classification: Classification::Regression,
            known_failure: None,
            script_errors: vec![],
            subtests: serde_json::json!({"message": "boom <x>"}),
        }];
        let report = WptReport {
            revision: "test".to_string(),
            summary: summarize(&results),
            results,
        };
        let xml = junit_xml(&report);
        assert!(xml.contains("tests=\"1\" failures=\"1\""));
        assert!(xml.contains("name=\"a&lt;&amp;&quot;&apos;\""));
        assert!(xml.contains("boom &lt;x&gt;"));
    }

    #[test]
    fn junit_marks_known_failures_without_skipping_and_reports_improvements() {
        let results = vec![
            WptResult {
                path: "dom/known.html".to_string(),
                area: "dom".to_string(),
                actual: ActualStatus::Timeout,
                classification: Classification::KnownFailure,
                known_failure: Some(known_failure(ActualStatus::Timeout)),
                script_errors: vec![],
                subtests: serde_json::json!([]),
            },
            WptResult {
                path: "css/improved.html".to_string(),
                area: "css".to_string(),
                actual: ActualStatus::Pass,
                classification: Classification::Improvement,
                known_failure: Some(known_failure(ActualStatus::Fail)),
                script_errors: vec![],
                subtests: serde_json::json!([]),
            },
        ];
        let report = WptReport {
            revision: "test".to_string(),
            summary: summarize(&results),
            results,
        };
        let xml = junit_xml(&report);
        assert!(xml.contains("failures=\"0\""));
        assert!(xml.contains("known-failure=\"true\""));
        assert!(
            xml.contains("KNOWN FAILURE [TIMEOUT] #123&amp;tracking: not implemented &lt;yet&gt;")
        );
        assert!(xml.contains("improvement=\"true\""));
        assert!(xml.contains("IMPROVEMENT: passed despite known FAIL failure"));
        assert!(!xml.contains("<skipped"));
    }

    #[test]
    fn revision_reports_keep_result_order_and_sanitize_area_filenames() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "omoikane-wpt-report-order-{}-{unique}",
            std::process::id()
        ));
        let results = [("dom/z.html", "dom"), ("svg:links/a.html", "svg:links")]
            .into_iter()
            .map(|(path, area)| WptResult {
                path: path.to_string(),
                area: area.to_string(),
                actual: ActualStatus::Pass,
                classification: Classification::Pass,
                known_failure: None,
                script_errors: vec![],
                subtests: serde_json::json!([]),
            })
            .collect::<Vec<_>>();
        let report = WptReport {
            revision: "fixed".to_string(),
            summary: summarize(&results),
            results,
        };

        write_revision_reports(&root, &report).unwrap();
        let revision_dir = root.join("fixed");
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(revision_dir.join("report.json")).unwrap()).unwrap();
        assert_eq!(json["results"][0]["path"], "dom/z.html");
        assert_eq!(json["results"][1]["path"], "svg:links/a.html");
        assert!(revision_dir.join("dom.json").is_file());
        assert!(revision_dir.join("svg_links.json").is_file());
        let area: WptAreaReport =
            serde_json::from_slice(&fs::read(revision_dir.join("svg_links.json")).unwrap())
                .unwrap();
        assert_eq!(area.area, "svg:links");
        assert_eq!(area.summary.total, 1);
        assert_eq!(area.results[0].path, "svg:links/a.html");
        let junit = junit_xml(&report);
        assert!(junit.find("dom/z.html").unwrap() < junit.find("svg:links/a.html").unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn revision_report_io_errors_are_returned() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "omoikane-wpt-report-file-{}-{unique}",
            std::process::id()
        ));
        fs::write(&root, b"not a directory").unwrap();
        let report = WptReport {
            revision: "fixed".to_string(),
            summary: summarize(&[]),
            results: Vec::new(),
        };

        let path = root.join("fixed").join("report.json");
        assert_eq!(
            write_revision_reports(&root, &report).unwrap_err().kind(),
            fs::create_dir_all(root.join("fixed")).unwrap_err().kind()
        );
        assert_eq!(
            read_revision_report(&root, "fixed").unwrap_err().kind(),
            fs::read(path).unwrap_err().kind()
        );
        fs::remove_file(root).unwrap();
    }
}
