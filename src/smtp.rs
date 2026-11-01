//! SMTP: one connection per send (no pool), bounded by a timeout.

use std::time::Duration;

use lettre::transport::smtp::authentication::{Credentials as SmtpCredentials, Mechanism};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::compose::Built;
use crate::config::{Auth, Credentials, Security};
use crate::error::{MailError, Result, smtp_auth_rejected};

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
    let mechanisms = match &creds.auth {
        Auth::Password(_) => vec![Mechanism::Plain, Mechanism::Login],
        // lettre builds the same `user=..^Aauth=Bearer ..^A^A` string as
        // `config::xoauth2_initial_response`.
        Auth::OAuth(_) => vec![Mechanism::Xoauth2],
    };
    Ok(builder
        .port(ep.port)
        .timeout(Some(SMTP_TIMEOUT))
        .authentication(mechanisms)
        .credentials(SmtpCredentials::new(
            creds.user.clone(),
            creds.auth.secret().expose().to_string(),
        ))
        .build())
}

fn smtp_error(creds: &Credentials, e: lettre::transport::smtp::Error) -> MailError {
    if smtp_auth_rejected(&e) {
        MailError::AuthRejected {
            protocol: "SMTP",
            user: creds.user.clone(),
            detail: e.to_string(),
        }
    } else {
        e.into()
    }
}

/// Send a built message: exactly the bytes [`Built::bytes`] returns (the same bytes are
/// appended to the sent folder afterwards).
pub async fn send(creds: &Credentials, built: &Built, bytes: &[u8]) -> Result<()> {
    let t = transport(creds)?;
    t.send_raw(built.message.envelope(), bytes)
        .await
        .map_err(|e| smtp_error(creds, e))?;
    Ok(())
}

/// Connect, EHLO and authenticate without sending anything: the doctor check.
pub async fn check(creds: &Credentials) -> Result<()> {
    let t = transport(creds)?;
    match t
        .test_connection()
        .await
        .map_err(|e| smtp_error(creds, e))?
    {
        true => Ok(()),
        false => Err(MailError::Smtp("server did not answer NOOP".into())),
    }
}
