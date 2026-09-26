//! GitHub Issue backend for sanitized, explicitly approved reports.

use crate::http::{Client, HttpRequest, Method};
use serde_json::{Value, json};

use super::{Repository, StoredReport, SubmissionBackend};

const MAX_ISSUE_PAGES: usize = 20;
const PAGE_SIZE: usize = 100;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// A sanitized GitHub API failure without token or response contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitHubFailure {
    /// A token was not supplied.
    MissingToken,
    /// TLS, transport, or response read failed.
    Transport,
    /// GitHub rejected the supplied credentials or permissions.
    Denied,
    /// GitHub rejected the request due to a rate limit.
    RateLimited,
    /// GitHub returned an unexpected status or malformed response.
    InvalidResponse,
    /// The bounded search could not cover all open Issues safely.
    SearchIncomplete,
}

impl std::fmt::Display for GitHubFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::MissingToken => "GitHub token is missing",
            Self::Transport => "GitHub transport failed",
            Self::Denied => "GitHub access denied",
            Self::RateLimited => "GitHub rate limit reached",
            Self::InvalidResponse => "GitHub response was invalid",
            Self::SearchIncomplete => "GitHub open-Issue search was incomplete",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for GitHubFailure {}

/// Minimal open Issue data needed for exact fingerprint matching.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteIssue {
    /// Repository-local Issue number.
    pub number: u64,
    /// Existing Issue body, including the Omoikane fingerprint marker.
    pub body: String,
    /// Whether GitHub returned a pull request through the Issues endpoint.
    pub is_pull_request: bool,
}

/// Mockable operations used by the GitHub Issue backend.
pub trait GitHubApi {
    /// Returns all open Issues or fails closed when the scan is incomplete.
    fn list_open_issues(
        &mut self,
        repository: &Repository,
    ) -> Result<Vec<RemoteIssue>, GitHubFailure>;

    /// Creates a new Issue and returns its number.
    fn create_issue(
        &mut self,
        repository: &Repository,
        title: &str,
        body: &str,
    ) -> Result<u64, GitHubFailure>;

    /// Replaces the managed report block while retaining unrelated body text.
    fn update_issue(
        &mut self,
        repository: &Repository,
        number: u64,
        body: &str,
    ) -> Result<(), GitHubFailure>;
}

/// Searches by repository and exact fingerprint before creating or updating.
pub struct GitHubIssueBackend<A> {
    api: A,
}

impl<A> GitHubIssueBackend<A> {
    /// Constructs a backend from a real or mock GitHub API implementation.
    pub const fn new(api: A) -> Self {
        Self { api }
    }

    /// Returns the API implementation, useful for inspecting a mock's calls.
    pub fn into_inner(self) -> A {
        self.api
    }
}

impl<A: GitHubApi> GitHubIssueBackend<A> {
    /// Creates an Issue or updates the matching open Issue idempotently.
    pub fn submit_report(
        &mut self,
        repository: &Repository,
        report: &StoredReport,
    ) -> Result<u64, GitHubFailure> {
        if !valid_fingerprint(&report.fingerprint) {
            return Err(GitHubFailure::InvalidResponse);
        }
        let start = start_marker(&report.fingerprint);
        let mut matching = self
            .api
            .list_open_issues(repository)?
            .into_iter()
            .filter(|issue| !issue.is_pull_request && issue.body.contains(&start));
        if let Some(issue) = matching.next() {
            let new_body = replace_report_block(&issue.body, report)?;
            if new_body != issue.body {
                self.api.update_issue(repository, issue.number, &new_body)?;
            }
            return Ok(issue.number);
        }
        let title = format!("Omoikane error: {}", report.error_code);
        let body = format!(
            "Automatically recorded Omoikane error.\n\n{}",
            report_block(report)
        );
        let number = self.api.create_issue(repository, &title, &body)?;
        if number == 0 {
            return Err(GitHubFailure::InvalidResponse);
        }
        Ok(number)
    }
}

impl<A: GitHubApi> SubmissionBackend for GitHubIssueBackend<A> {
    fn submit(&mut self, repository: &Repository, report: &StoredReport) -> Result<u64, ()> {
        self.submit_report(repository, report).map_err(|_| ())
    }
}

fn valid_fingerprint(fingerprint: &str) -> bool {
    fingerprint.len() == 67
        && fingerprint.starts_with("v1:")
        && fingerprint[3..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn start_marker(fingerprint: &str) -> String {
    format!("<!-- omoikane-error:{fingerprint}:start -->")
}

fn end_marker(fingerprint: &str) -> String {
    format!("<!-- omoikane-error:{fingerprint}:end -->")
}

fn report_block(report: &StoredReport) -> String {
    format!(
        "{}\n- Fingerprint: `{}`\n- Category: `{}`\n- Severity: `{}`\n- Code: `{}`\n- Message: {}\n- First seen (Unix ms): {}\n- Last seen (Unix ms): {}\n- Occurrences: {}\n- Version: `{}`\n- Commit: `{}`\n- Platform: `{}`\n- Surface: `{}`\n- Safe context: `{}`\n{}",
        start_marker(&report.fingerprint),
        report.fingerprint,
        report.category,
        report.severity,
        report.error_code,
        report.message,
        report.first_seen_ms,
        report.last_seen_ms,
        report.occurrences,
        report.version,
        report.commit.as_deref().unwrap_or("unknown"),
        report.platform,
        report.surface,
        report.context_json,
        end_marker(&report.fingerprint),
    )
}

fn replace_report_block(body: &str, report: &StoredReport) -> Result<String, GitHubFailure> {
    let start = start_marker(&report.fingerprint);
    let end = end_marker(&report.fingerprint);
    let before_start = body.find(&start).ok_or(GitHubFailure::InvalidResponse)?;
    let after_start = &body[before_start + start.len()..];
    let end_offset = after_start
        .find(&end)
        .ok_or(GitHubFailure::InvalidResponse)?;
    let after_end = before_start + start.len() + end_offset + end.len();
    Ok(format!(
        "{}{}{}",
        &body[..before_start],
        report_block(report),
        &body[after_end..]
    ))
}

/// REST client with a dedicated cookie jar and redirects disabled for API calls.
pub struct GitHubRestApi {
    token: String,
    client: Client,
}

impl GitHubRestApi {
    /// Constructs a GitHub REST client from an explicitly supplied token.
    pub fn new(token: impl Into<String>) -> Result<Self, GitHubFailure> {
        let token = token.into();
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(GitHubFailure::MissingToken);
        }
        let mut client = Client::new();
        client.set_max_redirects(0);
        client.set_user_agent(format!(
            "Omoikane-error-reporter/{}",
            env!("CARGO_PKG_VERSION")
        ));
        Ok(Self { token, client })
    }

    fn request(
        &mut self,
        method: Method,
        path: &str,
        body: Option<Value>,
        expected_status: u16,
    ) -> Result<Value, GitHubFailure> {
        let url = format!("https://api.github.com{path}");
        let url = url.parse().map_err(|_| GitHubFailure::InvalidResponse)?;
        let mut request = HttpRequest::new(method, url);
        request.set_header("Accept", "application/vnd.github+json");
        request.set_header("Accept-Encoding", "identity");
        request.set_header("X-GitHub-Api-Version", "2022-11-28");
        request.set_header("Authorization", format!("Bearer {}", self.token));
        if let Some(body) = body {
            request.set_header("Content-Type", "application/json");
            request
                .set_body(serde_json::to_vec(&body).map_err(|_| GitHubFailure::InvalidResponse)?);
        }
        let response = self
            .client
            .send(request)
            .map_err(|_| GitHubFailure::Transport)?;
        match response.status_code() {
            code if code == expected_status => {}
            401 => return Err(GitHubFailure::Denied),
            403 | 429
                if response.header("retry-after").is_some()
                    || response.header("x-ratelimit-remaining") == Some("0")
                    || response.status_code() == 429 =>
            {
                return Err(GitHubFailure::RateLimited);
            }
            403 => return Err(GitHubFailure::Denied),
            _ => return Err(GitHubFailure::InvalidResponse),
        }
        if response.body().len() > MAX_RESPONSE_BYTES {
            return Err(GitHubFailure::InvalidResponse);
        }
        serde_json::from_slice(response.body()).map_err(|_| GitHubFailure::InvalidResponse)
    }
}

impl GitHubApi for GitHubRestApi {
    fn list_open_issues(
        &mut self,
        repository: &Repository,
    ) -> Result<Vec<RemoteIssue>, GitHubFailure> {
        let mut issues = Vec::new();
        for page in 1..=MAX_ISSUE_PAGES {
            let path = format!(
                "/repos/{}/{}/issues?state=open&per_page={PAGE_SIZE}&page={page}",
                repository.owner(),
                repository.name()
            );
            let value = self.request(Method::Get, &path, None, 200)?;
            let items = value.as_array().ok_or(GitHubFailure::InvalidResponse)?;
            if items.len() > PAGE_SIZE {
                return Err(GitHubFailure::InvalidResponse);
            }
            for item in items {
                let number = item
                    .get("number")
                    .and_then(Value::as_u64)
                    .filter(|number| *number > 0)
                    .ok_or(GitHubFailure::InvalidResponse)?;
                let body = item
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                issues.push(RemoteIssue {
                    number,
                    body,
                    is_pull_request: item.get("pull_request").is_some(),
                });
            }
            if items.len() < PAGE_SIZE {
                return Ok(issues);
            }
        }
        Err(GitHubFailure::SearchIncomplete)
    }

    fn create_issue(
        &mut self,
        repository: &Repository,
        title: &str,
        body: &str,
    ) -> Result<u64, GitHubFailure> {
        let path = format!("/repos/{}/{}/issues", repository.owner(), repository.name());
        self.request(
            Method::Post,
            &path,
            Some(json!({"title": title, "body": body})),
            201,
        )?
        .get("number")
        .and_then(Value::as_u64)
        .filter(|number| *number > 0)
        .ok_or(GitHubFailure::InvalidResponse)
    }

    fn update_issue(
        &mut self,
        repository: &Repository,
        number: u64,
        body: &str,
    ) -> Result<(), GitHubFailure> {
        let path = format!(
            "/repos/{}/{}/issues/{number}",
            repository.owner(),
            repository.name()
        );
        self.request(Method::Patch, &path, Some(json!({"body": body})), 200)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::error_reporting::{
        ErrorCategory, ErrorCode, ErrorSeverity, EventStore, ExecutionSurface, ManualSubmission,
        RawEvent, ReporterConfig, RetentionPolicy, SubmissionApproval,
    };

    static NEXT_DB: AtomicU64 = AtomicU64::new(0);

    #[derive(Default)]
    struct MockApi {
        issues: Vec<RemoteIssue>,
        creates: usize,
        updates: usize,
        scans: usize,
        fail_scan: bool,
    }

    impl GitHubApi for MockApi {
        fn list_open_issues(&mut self, _: &Repository) -> Result<Vec<RemoteIssue>, GitHubFailure> {
            self.scans += 1;
            if self.fail_scan {
                return Err(GitHubFailure::SearchIncomplete);
            }
            Ok(self.issues.clone())
        }

        fn create_issue(
            &mut self,
            _: &Repository,
            _: &str,
            body: &str,
        ) -> Result<u64, GitHubFailure> {
            self.creates += 1;
            self.issues.push(RemoteIssue {
                number: 42,
                body: body.to_owned(),
                is_pull_request: false,
            });
            Ok(42)
        }

        fn update_issue(
            &mut self,
            _: &Repository,
            number: u64,
            body: &str,
        ) -> Result<(), GitHubFailure> {
            self.updates += 1;
            self.issues
                .iter_mut()
                .find(|issue| issue.number == number)
                .unwrap()
                .body = body.to_owned();
            Ok(())
        }
    }

    fn report() -> StoredReport {
        StoredReport {
            fingerprint: format!("v1:{}", "a".repeat(64)),
            category: "javascript".to_owned(),
            severity: "error".to_owned(),
            error_code: "SCRIPT_FAILED".to_owned(),
            message: "JavaScript execution failed".to_owned(),
            context_json: r#"{"operation":"execute"}"#.to_owned(),
            first_seen_ms: 1_000,
            last_seen_ms: 2_000,
            occurrences: 2,
            version: "0.4.0".to_owned(),
            commit: None,
            platform: "linux/aarch64".to_owned(),
            surface: "headless".to_owned(),
            submission_status: "pending".to_owned(),
            issue_url: None,
        }
    }

    #[test]
    fn creates_then_reuses_open_issue_and_updates_only_its_report_block() {
        let repository = Repository::parse("owner/repo").unwrap();
        let mut backend = GitHubIssueBackend::new(MockApi::default());
        let mut row = report();
        assert_eq!(backend.submit_report(&repository, &row), Ok(42));
        assert_eq!(backend.api.creates, 1);
        assert!(backend.api.issues[0].body.contains(&row.fingerprint));
        backend.api.issues[0]
            .body
            .push_str("\n\nUser note remains.");
        row.occurrences = 3;
        row.last_seen_ms = 3_000;
        assert_eq!(backend.submit_report(&repository, &row), Ok(42));
        assert_eq!(backend.api.creates, 1);
        assert_eq!(backend.api.updates, 1);
        assert!(backend.api.issues[0].body.contains("Occurrences: 3"));
        assert!(
            backend.api.issues[0]
                .body
                .contains("Last seen (Unix ms): 3000")
        );
        assert!(backend.api.issues[0].body.contains("User note remains."));
        assert_eq!(backend.submit_report(&repository, &row), Ok(42));
        assert_eq!(backend.api.updates, 1);
        assert_eq!(backend.api.scans, 3);
    }

    #[test]
    fn ignores_pull_requests_and_fails_closed_on_incomplete_or_broken_search() {
        let repository = Repository::parse("owner/repo").unwrap();
        let row = report();
        let mut backend = GitHubIssueBackend::new(MockApi {
            issues: vec![RemoteIssue {
                number: 10,
                body: report_block(&row),
                is_pull_request: true,
            }],
            ..MockApi::default()
        });
        assert_eq!(backend.submit_report(&repository, &row), Ok(42));
        assert_eq!(backend.api.creates, 1);

        let mut incomplete = GitHubIssueBackend::new(MockApi {
            fail_scan: true,
            ..MockApi::default()
        });
        assert_eq!(
            incomplete.submit_report(&repository, &row),
            Err(GitHubFailure::SearchIncomplete)
        );
        assert_eq!(incomplete.api.creates, 0);

        let mut malformed = GitHubIssueBackend::new(MockApi {
            issues: vec![RemoteIssue {
                number: 11,
                body: start_marker(&row.fingerprint),
                is_pull_request: false,
            }],
            ..MockApi::default()
        });
        assert_eq!(
            malformed.submit_report(&repository, &row),
            Err(GitHubFailure::InvalidResponse)
        );
        assert_eq!(malformed.api.creates, 0);
    }

    #[test]
    fn token_and_fingerprint_errors_do_not_echo_inputs() {
        assert_eq!(
            GitHubRestApi::new("secret\nHeader: injected").err(),
            Some(GitHubFailure::MissingToken)
        );
        assert!(!GitHubFailure::MissingToken.to_string().contains("secret"));
        let repository = Repository::parse("owner/repo").unwrap();
        let mut backend = GitHubIssueBackend::new(MockApi::default());
        let mut row = report();
        row.fingerprint = "PRIVATE\n".to_owned();
        assert_eq!(
            backend.submit_report(&repository, &row),
            Err(GitHubFailure::InvalidResponse)
        );
        assert_eq!(backend.api.scans, 0);
    }

    #[test]
    fn manual_submission_saves_issue_url_and_reuses_it_after_new_occurrence() {
        let path = std::env::temp_dir().join(format!(
            "omoikane-github-backend-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let mut store =
            EventStore::open(path.join("events.sqlite"), RetentionPolicy::default()).unwrap();
        let safe = RawEvent::new(
            ErrorCategory::JavaScript,
            ErrorSeverity::Error,
            ErrorCode::new("GITHUB_REUSE_TEST").unwrap(),
            ExecutionSurface::Headless,
            "Bearer PRIVATE_SECRET /home/user/private.js?key=HIDDEN_SECRET",
            &[
                ("operation", "execute"),
                ("url", "https://private.example/?key=HIDDEN_SECRET"),
            ],
        )
        .sanitize();
        let config = ReporterConfig::from_values(Some("submit"), Some("owner/repo")).unwrap();
        store.record_at(&safe, 1_000).unwrap();
        let mut backend = GitHubIssueBackend::new(MockApi::default());
        let manual = ManualSubmission::new(&config, &store);
        let url = manual
            .submit_selected(
                safe.fingerprint(),
                SubmissionApproval::Confirmed,
                &mut backend,
            )
            .unwrap();
        assert_eq!(url, "https://github.com/owner/repo/issues/42");
        let row = store.get(safe.fingerprint()).unwrap().unwrap();
        assert_eq!(row.submission_status, "submitted");
        assert_eq!(row.issue_url.as_deref(), Some(url.as_str()));
        drop(manual);
        store.record_at(&safe, 2_000).unwrap();
        assert_eq!(
            store
                .get(safe.fingerprint())
                .unwrap()
                .unwrap()
                .submission_status,
            "pending"
        );
        let manual = ManualSubmission::new(&config, &store);
        assert_eq!(
            manual
                .submit_selected(
                    safe.fingerprint(),
                    SubmissionApproval::Confirmed,
                    &mut backend,
                )
                .unwrap(),
            url
        );
        assert_eq!(backend.api.creates, 1);
        assert_eq!(backend.api.updates, 1);
        assert!(backend.api.issues[0].body.contains("Occurrences: 2"));
        assert!(!backend.api.issues[0].body.contains("PRIVATE_SECRET"));
        assert!(!backend.api.issues[0].body.contains("HIDDEN_SECRET"));
        assert!(!backend.api.issues[0].body.contains("private.example"));
        drop(manual);
        drop(store);
        fs::remove_dir_all(path).unwrap();
    }
}
