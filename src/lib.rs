//! emperor-mail: an MCP server for email over IMAP and SMTP, built on
//! [emperor-mcp](https://github.com/yarenty/emperor-mcp).
//!
//! Six tools: `list_folders`, `search`, `read_message`, `read_attachment`, `draft`, `send`.
//! Credentials come from forwarded request headers (shared HTTP server) or the environment (one
//! mailbox); see [`config`]. Sending is off unless the operator allows it; see [`policy`].

#![warn(missing_docs)]

pub mod compose;
pub mod config;
pub mod doctor;
pub mod error;
pub mod imap;
pub mod message;
pub mod policy;
pub mod search;
pub mod server;
pub mod smtp;

pub use config::{Credentials, Settings, forward_config};
pub use error::MailError;
pub use policy::SendPolicy;
pub use server::MailServer;

/// Initialise logging to **stderr** (stdout carries the protocol in stdio mode), filtered by
/// `RUST_LOG`, default `info`.
pub fn init_tracing() {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .try_init();
}
