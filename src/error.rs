//! One error type for every tool. Messages are written for the model and the operator: they say
//! what went wrong and what to do, and never contain a password.

use std::time::Duration;

/// Everything that can go wrong in a tool call or a doctor check.
#[derive(Debug, thiserror::Error)]
pub enum MailError {
    /// No credentials (or no host) from either source.
    #[error("missing credentials: {0}")]
    MissingCredentials(String),
    /// A setting is present but unusable (bad port, unknown security mode, ...).
    #[error("configuration: {0}")]
    Config(String),
    /// The caller's arguments are invalid.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// TCP or TLS connection failed.
    #[error("connection to {target} failed: {detail}")]
    Connect {
        /// `host:port`.
        target: String,
        /// What happened.
        detail: String,
    },
    /// The server refused the login.
    #[error("login as {user} failed: {detail}")]
    Login {
        /// The user name (never the password).
        user: String,
        /// The server's answer.
        detail: String,
    },
    /// An IMAP command failed.
    #[error("IMAP: {0}")]
    Imap(String),
    /// An SMTP exchange failed.
    #[error("SMTP: {0}")]
    Smtp(String),
    /// The folder, message or attachment does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The send policy refused the message.
    #[error("sending refused: {0}")]
    SendRefused(String),
    /// The upstream did not answer in time.
    #[error("{what} timed out after {after:?}")]
    Timeout {
        /// What timed out.
        what: &'static str,
        /// The limit.
        after: Duration,
    },
    /// A message or attachment could not be decoded.
    #[error("could not decode: {0}")]
    Decode(String),
}

impl From<async_imap::error::Error> for MailError {
    fn from(e: async_imap::error::Error) -> Self {
        Self::Imap(e.to_string())
    }
}

impl From<lettre::transport::smtp::Error> for MailError {
    fn from(e: lettre::transport::smtp::Error) -> Self {
        Self::Smtp(e.to_string())
    }
}

/// Result alias.
pub type Result<T> = std::result::Result<T, MailError>;
