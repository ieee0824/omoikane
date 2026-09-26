use super::{
    ConfigError, ErrorCategory, ErrorCode, ErrorSeverity, ExecutionSurface, RawEvent,
    ReporterConfig, ReportingMode, Repository, fingerprint,
};

fn safe_event(message: &str, context: &[(&str, &str)]) -> super::SafeEvent {
    RawEvent::new(
        ErrorCategory::JavaScript,
        ErrorSeverity::Error,
        ErrorCode::new("SCRIPT_EXECUTION_FAILED").unwrap(),
        ExecutionSurface::Headless,
        message,
        context,
    )
    .sanitize()
}

#[test]
fn configuration_defaults_to_off_and_requires_explicit_submit_target() {
    assert_eq!(ReporterConfig::default().mode(), ReportingMode::Off);
    assert!(!ReporterConfig::default().records_locally());
    assert!(!ReporterConfig::default().permits_submission());
    assert_eq!(
        ReporterConfig::from_values(None, None).unwrap().mode(),
        ReportingMode::Off
    );
    assert_eq!(
        ReporterConfig::from_values(Some(" ReCoRd-OnLy "), None)
            .unwrap()
            .mode(),
        ReportingMode::RecordOnly
    );
    let record_only = ReporterConfig::from_values(Some("record-only"), None).unwrap();
    assert!(record_only.records_locally());
    assert!(!record_only.permits_submission());
    assert_eq!(
        ReporterConfig::from_values(Some("submit"), None),
        Err(ConfigError::MissingRepository)
    );
    let config = ReporterConfig::from_values(Some("submit"), Some("ieee0824/omoikane"))
        .expect("explicit target");
    assert_eq!(config.mode(), ReportingMode::Submit);
    assert!(config.records_locally());
    assert!(config.permits_submission());
    assert_eq!(config.repository().unwrap().owner(), "ieee0824");
    assert_eq!(config.repository().unwrap().name(), "omoikane");
    assert_eq!(
        ReporterConfig::from_values(None, Some("ieee0824/omoikane"))
            .unwrap()
            .mode(),
        ReportingMode::Off,
        "repository alone must not enable reporting"
    );
    assert!(
        !ReporterConfig::from_values(None, Some("ieee0824/omoikane"))
            .unwrap()
            .permits_submission()
    );
}

#[test]
fn configuration_rejects_invalid_values_without_echoing_them() {
    assert_eq!(
        ReporterConfig::from_values(Some("send-now?token=PRIVATE"), None),
        Err(ConfigError::InvalidMode)
    );
    for invalid in [
        "https://github.com/owner/repo",
        "owner/repo?token=PRIVATE",
        "owner/repo/extra",
        "owner/ repo",
        "-owner/repo",
        "owner/",
        "",
    ] {
        let error = Repository::parse(invalid).unwrap_err();
        assert_eq!(error, ConfigError::InvalidRepository);
        assert!(!error.to_string().contains("PRIVATE"));
    }
}

#[test]
fn configuration_env_subprocess_probe() {
    if std::env::var_os("OMOIKANE_CONFIG_PROBE").is_none() {
        return;
    }
    println!(
        "CONFIG_RESULT={:?}",
        ReporterConfig::from_env().map(|config| config.mode())
    );
}

#[test]
fn environment_configuration_is_off_by_default_and_fails_closed() {
    fn probe(mode: Option<&str>, repository: Option<&str>) -> String {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "error_reporting::tests::configuration_env_subprocess_probe",
                "--nocapture",
            ])
            .env("OMOIKANE_CONFIG_PROBE", "1")
            .env_remove("OMOIKANE_ERROR_REPORT_MODE")
            .env_remove("OMOIKANE_ERROR_REPORT_REPOSITORY");
        if let Some(mode) = mode {
            command.env("OMOIKANE_ERROR_REPORT_MODE", mode);
        }
        if let Some(repository) = repository {
            command.env("OMOIKANE_ERROR_REPORT_REPOSITORY", repository);
        }
        let output = command
            .output()
            .expect("run config probe in another process");
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    }

    assert!(probe(None, None).contains("CONFIG_RESULT=Ok(Off)"));
    assert!(probe(Some("record-only"), None).contains("CONFIG_RESULT=Ok(RecordOnly)"));
    assert!(probe(Some("submit"), None).contains("CONFIG_RESULT=Err(MissingRepository)"));
    let invalid = probe(Some("submit"), Some("owner/repo?token=SECRET"));
    assert!(invalid.contains("CONFIG_RESULT=Err(InvalidRepository)"));
    assert!(!invalid.contains("SECRET"));
    assert!(probe(Some("submit"), Some("owner/repo")).contains("CONFIG_RESULT=Ok(Submit)"));
}

#[test]
fn error_code_must_be_stable_and_source_defined() {
    for invalid in [
        "",
        "lower_case",
        "A-B",
        "1START",
        "URL?secret",
        "A".repeat(65).leak(),
    ] {
        assert!(ErrorCode::new(invalid).is_err());
    }
    assert_eq!(
        ErrorCode::new("HTTP_TLS_FAILURE_2").unwrap().as_str(),
        "HTTP_TLS_FAILURE_2"
    );
}

#[test]
fn raw_secrets_never_enter_serialized_or_displayed_event() {
    let secrets = [
        "https://example.test/path?session=QUERY_SECRET#FRAGMENT_SECRET",
        "Cookie: session=COOKIE_SECRET",
        "Authorization: Bearer AUTH_SECRET",
        "request body=BODY_SECRET",
        "<main>DOM_SECRET</main>",
        "eval('JS_SOURCE_SECRET')",
        "/home/alice/private/PATH_SECRET",
        "HOME=ENV_SECRET",
        "ghp_TOKEN_SECRET",
    ];
    for secret in secrets {
        let event = safe_event(
            secret,
            &[("operation", "execute"), ("request_body", secret)],
        );
        let serialized = serde_json::to_string(&event).unwrap();
        let displayed = format!("{event:?}");
        assert!(
            !serialized.contains(secret),
            "serialized event leaked raw input"
        );
        assert!(!displayed.contains(secret), "debug output leaked raw input");
        assert_eq!(event.message(), "Error details withheld");
        assert_eq!(event.context().values().get("operation"), Some(&"execute"));
        assert!(!event.context().values().contains_key("request_body"));
    }
}

#[test]
fn combined_input_is_reduced_to_an_explicit_allowlist() {
    let secret = "https://site.test/?key=SECRET#fragment Cookie: c=SECRET /tmp/SECRET";
    let event = safe_event(
        secret,
        &[
            ("url", secret),
            ("authorization", secret),
            ("dom_text", secret),
            ("operation", secret),
            ("resource", "script"),
            ("failure_kind", "timeout"),
            ("http_status_class", "5xx"),
        ],
    );
    let json = serde_json::to_string(&event).unwrap();
    assert!(!json.contains("SECRET"));
    assert!(!json.contains("site.test"));
    assert_eq!(event.context().values().len(), 3);
    assert_eq!(
        event.context().values().get("failure_kind"),
        Some(&"timeout")
    );
    assert_eq!(event.context().values().get("resource"), Some(&"script"));
    assert_eq!(
        event.context().values().get("http_status_class"),
        Some(&"5xx")
    );
    assert_eq!(event.version(), env!("CARGO_PKG_VERSION"));
    assert!(event.platform().contains('/'));
}

#[test]
fn sqlite_and_display_payloads_contain_only_sanitized_data() {
    let raw = "Authorization: Bearer TOKEN_SECRET; /home/user/private.js?key=QUERY_SECRET";
    let event = safe_event(raw, &[("authorization", raw), ("operation", "execute")]);
    let payload = serde_json::to_string(&event).unwrap();
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection
        .execute("CREATE TABLE events (payload TEXT NOT NULL)", [])
        .unwrap();
    connection
        .execute("INSERT INTO events (payload) VALUES (?1)", [&payload])
        .unwrap();
    let stored: String = connection
        .query_row("SELECT payload FROM events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, payload);
    for exposed in [stored, format!("{event:?}")] {
        assert!(!exposed.contains("TOKEN_SECRET"));
        assert!(!exposed.contains("QUERY_SECRET"));
        assert!(!exposed.contains("/home/user"));
        assert!(!exposed.contains("Authorization"));
    }
}

#[test]
fn fingerprint_uses_only_canonical_safe_fields() {
    let first = safe_event(
        "https://one.test/?token=ALPHA /home/a/secret",
        &[
            ("resource", "script"),
            ("operation", "execute"),
            ("cookie", "ALPHA"),
        ],
    );
    let second = safe_event(
        "https://two.test/?token=BETA /home/b/secret",
        &[
            ("operation", "execute"),
            ("resource", "script"),
            ("cookie", "BETA"),
        ],
    );
    assert_eq!(first.fingerprint(), second.fingerprint());
    assert!(first.fingerprint().starts_with("v1:"));
    assert_eq!(first.fingerprint().len(), 67);
    let input = fingerprint::canonical_input(first.category(), first.code(), first.context());
    for forbidden in ["ALPHA", "BETA", "one.test", "/home/", "cookie", "token"] {
        assert!(
            !input
                .windows(forbidden.len())
                .any(|window| window == forbidden.as_bytes())
        );
    }
    let other_cause = RawEvent::new(
        ErrorCategory::JavaScript,
        ErrorSeverity::Error,
        ErrorCode::new("MODULE_LOAD_FAILED").unwrap(),
        ExecutionSurface::Headless,
        "https://one.test/?token=ALPHA",
        &[("operation", "execute"), ("resource", "script")],
    )
    .sanitize();
    assert_ne!(first.fingerprint(), other_cause.fingerprint());
    let other_context = safe_event("Operation timed out", &[("failure_kind", "timeout")]);
    assert_ne!(first.fingerprint(), other_context.fingerprint());
    let other_category = RawEvent::new(
        ErrorCategory::Css,
        ErrorSeverity::Error,
        ErrorCode::new("SCRIPT_EXECUTION_FAILED").unwrap(),
        ExecutionSurface::Headless,
        "https://one.test/?token=ALPHA",
        &[("operation", "execute"), ("resource", "script")],
    )
    .sanitize();
    assert_ne!(first.fingerprint(), other_category.fingerprint());
}

#[test]
fn fingerprint_subprocess_probe() {
    if std::env::var_os("OMOIKANE_FINGERPRINT_PROBE").is_none() {
        return;
    }
    let event = safe_event(
        "https://site.test/?secret=HIDDEN",
        &[("resource", "script"), ("operation", "execute")],
    );
    println!("FINGERPRINT={}", event.fingerprint());
}

#[test]
fn fingerprint_is_stable_across_processes() {
    let expected = safe_event(
        "https://other.test/?different=SECRET",
        &[("operation", "execute"), ("resource", "script")],
    );
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "error_reporting::tests::fingerprint_subprocess_probe",
            "--nocapture",
        ])
        .env("OMOIKANE_FINGERPRINT_PROBE", "1")
        .output()
        .expect("run fingerprint probe in another process");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let actual = stdout
        .split("FINGERPRINT=")
        .nth(1)
        .expect("probe emitted fingerprint");
    assert_eq!(
        &actual[..expected.fingerprint().len()],
        expected.fingerprint()
    );
}
