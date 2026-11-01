//! `emperor-mail doctor`: checks configuration, credentials (never printed), the IMAP and SMTP
//! servers, the send policy, the listen address and the tool budget.

use std::net::SocketAddr;
use std::time::Duration;

use emperor_mcp::doctor::{Check, Report, env_set, port_free, tcp_reachable, tool_budget};

use crate::config::{Credentials, Field, Settings};
use crate::policy::SendPolicy;
use crate::server::MailServer;
use crate::{imap, smtp};

const REACH_TIMEOUT: Duration = Duration::from_secs(5);

/// What the doctor needs to know about how the server would run.
#[derive(Debug, Clone)]
pub struct Plan {
    /// HTTP bind address (`None` in stdio mode).
    pub bind: Option<SocketAddr>,
    /// Send policy from the flags.
    pub policy: SendPolicy,
}

/// Run every check.
pub async fn run(env: &Settings, plan: &Plan) -> Report {
    let mut r = Report::new("emperor-mail", env!("CARGO_PKG_VERSION"));
    let http = plan.bind.is_some();
    let per_request = |what: &str, header: &str| {
        Check::warn(
            what,
            format!("not set; credentials come per request ({header})"),
        )
    };

    // Credentials.
    if env.has_login() || !http {
        r.push(env_set(Field::User.env(), false));
        r.push(env_set(Field::Password.env(), true));
    } else {
        r.push(per_request(
            "MAIL_USER/MAIL_PASSWORD",
            "X-Mail-User / X-Mail-Password headers",
        ));
    }

    // IMAP host.
    let imap_host = env.get(Field::ImapHost);
    match imap_host {
        Some(_) => r.push(env_set(Field::ImapHost.env(), false)),
        None if http => r.push(per_request(Field::ImapHost.env(), "X-Imap-Host header")),
        None => r.push(env_set(Field::ImapHost.env(), false)),
    };

    // Resolve the env source alone: the same code path tool calls use.
    let creds = Credentials::resolve(env, &Settings::default());
    let endpoints = match &creds {
        Ok(c) => Some((c.imap.clone(), c.smtp.clone())),
        Err(_) => None,
    };
    if let Err(e) = &creds
        && (env.has_login() || imap_host.is_some())
    {
        r.push(Check::fail("configuration", e.to_string()));
    }

    // IMAP reachable + login.
    if let Some((ep, _)) = &endpoints {
        r.push(tcp_reachable("imap reachable", &ep.host, ep.port, REACH_TIMEOUT).await);
    }
    match &creds {
        Ok(c) => r.push(match imap::check_login(c).await {
            Ok(n) => Check::pass(
                "imap login",
                format!(
                    "logged in to {} as {}; INBOX has {n} message(s)",
                    c.imap, c.user
                ),
            ),
            Err(e) => Check::fail("imap login", e.to_string()),
        }),
        Err(_) if http => r.push(Check::warn(
            "imap login",
            "skipped: credentials come per request",
        )),
        Err(_) => r.push(Check::fail("imap login", "skipped: no credentials")),
    };

    // SMTP.
    match endpoints.as_ref().and_then(|(_, smtp)| smtp.clone()) {
        Some(ep) => {
            r.push(tcp_reachable("smtp reachable", &ep.host, ep.port, REACH_TIMEOUT).await);
            if plan.policy.enabled
                && let Ok(c) = &creds
            {
                r.push(match smtp::check(c).await {
                    Ok(()) => Check::pass(
                        "smtp login",
                        format!("EHLO and AUTH accepted by {ep} (nothing sent)"),
                    ),
                    Err(e) => Check::fail("smtp login", e.to_string()),
                });
            }
        }
        None if plan.policy.enabled && !http => {
            r.push(Check::fail(
                Field::SmtpHost.env(),
                "not set, but --allow-send needs an SMTP server",
            ));
        }
        None if plan.policy.enabled => {
            r.push(per_request(Field::SmtpHost.env(), "X-Smtp-Host header"));
        }
        None => {
            r.push(Check::pass(
                Field::SmtpHost.env(),
                "not set (not needed: sending is off)",
            ));
        }
    }

    // Send policy.
    r.push(if plan.policy.enabled && plan.policy.patterns.is_empty() {
        Check::warn("send policy", plan.policy.summary())
    } else {
        Check::pass("send policy", plan.policy.summary())
    });

    // Listen address.
    if let Some(bind) = plan.bind {
        r.push(port_free(bind));
    }

    r.push(tool_budget(MailServer::tool_names().len()));
    r
}
