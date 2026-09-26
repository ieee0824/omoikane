use std::fmt;

/// Controls whether general errors are ignored, recorded, or eligible for submission.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReportingMode {
    /// Disable general-error reporting, including persistence and submission.
    #[default]
    Off,
    /// Permit local recording but never submit to GitHub.
    RecordOnly,
    /// Permit submission to the explicitly configured repository.
    Submit,
}

impl ReportingMode {
    /// Parses `off`, `record-only`, or `submit` (ASCII case-insensitive).
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "record-only" => Ok(Self::RecordOnly),
            "submit" => Ok(Self::Submit),
            _ => Err(ConfigError::InvalidMode),
        }
    }
}

/// A validated GitHub `owner/repository` destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repository {
    owner: String,
    name: String,
}

impl Repository {
    /// Accepts a repository slug, never a URL or an implicit default target.
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        let (owner, name) = value
            .split_once('/')
            .ok_or(ConfigError::InvalidRepository)?;
        let valid_owner = !owner.is_empty()
            && owner.len() <= 39
            && owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !owner.starts_with('-')
            && !owner.ends_with('-');
        let valid_name = !name.is_empty()
            && name.len() <= 100
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        if !valid_owner || !valid_name {
            return Err(ConfigError::InvalidRepository);
        }
        Ok(Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    /// Returns the repository owner.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Returns the repository name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// General-error reporter settings; constructing this type performs no I/O.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReporterConfig {
    mode: ReportingMode,
    repository: Option<Repository>,
}

impl ReporterConfig {
    /// Parses explicit values. An absent mode defaults to [`ReportingMode::Off`].
    ///
    /// `submit` requires an explicit repository slug. A repository never turns
    /// on submission by itself.
    pub fn from_values(mode: Option<&str>, repository: Option<&str>) -> Result<Self, ConfigError> {
        let mode = mode
            .map(ReportingMode::parse)
            .transpose()?
            .unwrap_or_default();
        let repository = repository.map(Repository::parse).transpose()?;
        if mode == ReportingMode::Submit && repository.is_none() {
            return Err(ConfigError::MissingRepository);
        }
        Ok(Self { mode, repository })
    }

    /// Reads `OMOIKANE_ERROR_REPORT_MODE` and `OMOIKANE_ERROR_REPORT_REPOSITORY`.
    ///
    /// Invalid or non-Unicode values fail closed. No database is opened and no
    /// network request is sent while reading these settings.
    pub fn from_env() -> Result<Self, ConfigError> {
        fn read(name: &str) -> Result<Option<String>, ConfigError> {
            match std::env::var(name) {
                Ok(value) => Ok(Some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::NonUnicode),
            }
        }
        let mode = read("OMOIKANE_ERROR_REPORT_MODE")?;
        let repository = read("OMOIKANE_ERROR_REPORT_REPOSITORY")?;
        Self::from_values(mode.as_deref(), repository.as_deref())
    }

    /// Returns the selected mode.
    pub const fn mode(&self) -> ReportingMode {
        self.mode
    }

    /// Whether a storage backend may open or write the general-error database.
    pub const fn records_locally(&self) -> bool {
        !matches!(self.mode, ReportingMode::Off)
    }

    /// Whether a submitter may consider this configuration for GitHub delivery.
    /// A later submission policy still controls confirmation and rate limits.
    pub const fn permits_submission(&self) -> bool {
        matches!(self.mode, ReportingMode::Submit) && self.repository.is_some()
    }

    /// Returns the explicit repository, if one was configured.
    pub fn repository(&self) -> Option<&Repository> {
        self.repository.as_ref()
    }
}

/// A settings error without echoing potentially sensitive input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    /// Reporting mode is not one of the supported values.
    InvalidMode,
    /// Repository is not a valid `owner/name` slug.
    InvalidRepository,
    /// Submission was requested without an explicit repository.
    MissingRepository,
    /// An environment setting was not valid Unicode.
    NonUnicode,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidMode => "invalid error-reporting mode",
            Self::InvalidRepository => "invalid error-reporting repository",
            Self::MissingRepository => "submit mode requires an explicit repository",
            Self::NonUnicode => "error-reporting setting is not Unicode",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ConfigError {}
