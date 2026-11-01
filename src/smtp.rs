//! SMTP: one connection per send (no pool), bounded by a timeout.

use std::time::Duration;

use lettre::transport::smtp::authentication::Credentials as SmtpCredentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::compose::Built;
use crate::config::{Credentials, Security};
use crate::error::{MailError, Result};

/// Per-command SMTP timeout.
pub const SMTP_TIMEOUT: Duration = Duration::from_secs(30);

fn transport(creds: &Credentials) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let ep = creds.smtp()?;
    let builder = match ep.security {
        Security::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&ep.host)?,
        Security::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&ep.host)?,
        Security::Plain => {
            if !ep.is_loopback() {
                return Err(MailError::Config(
                    "SMTP security `plain` is allowed only for a loopback host".into(),
                ));
            }
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&ep.host)
        }
    };
    Ok(builder
        .port(ep.port)
        .timeout(Some(SMTP_TIMEOUT))
        .credentials(SmtpCredentials::new(
            creds.user.clone(),
            creds.password.expose().to_string(),
        ))
        .build())
}

/// Send a built message.
pub async fn send(creds: &Credentials, built: &Built) -> Result<()> {
    let t = transport(creds)?;
    t.send(built.message.clone()).await?;
    Ok(())
}

/// Connect, EHLO and authenticate without sending anything: the doctor check.
pub async fn check(creds: &Credentials) -> Result<()> {
    let t = transport(creds)?;
    match t.test_connection().await? {
        true => Ok(()),
        false => Err(MailError::Smtp("server did not answer NOOP".into())),
    }
}
