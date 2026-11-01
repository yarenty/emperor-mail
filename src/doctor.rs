//! `emperor-mail doctor`: checks configuration, credentials (never printed), the IMAP and SMTP
//! servers, the mode (which write tools are listed), the send policy, the listen address and the
//! tool budget.

use std::net::SocketAddr;
use std::time::Duration;

use emperor_mcp::doctor::{Check, Report, env_set, port_free, tcp_reachable, tool_budget};

use crate::config::{Credentials, Field, Settings};
use crate::policy::{FromPolicy, Mode, SendPolicy};
use crate::server::MailServer;
use crate::{imap, smtp};

const REACH_TIMEOUT: Duration = Duration::from_secs(5);

/// What the doctor needs to know about how the server would run.
#[derive(Debug, Clone)]
pub struct Plan {
    /// HTTP bind address (`None` in stdio mode).
    pub bind: Option<SocketAddr>,
    /// Read-only, drafts, or drafts and send (`--writable`, `--allow-send`).
    pub mode: Mode,
    /// Send policy from the flags.
    pub policy: SendPolicy,
    /// `--from-allow` patterns.
    pub from_policy: FromPolicy,
    /// Sent messages are copied to the sent folder.
    pub sent_copy: bool,
    /// `--pin-servers`.
    pub pin_servers: bool,
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
        if env.get(Field::OauthToken).is_some() {
            r.push(env_set(Field::OauthToken.env(), true));
        } else {
            r.push(env_set(Field::Password.env(), true));
        }
    } else {
        r.push(per_request(
            "MAIL_USER/MAIL_PASSWORD",
            "X-Mail-User with X-Mail-Password or X-Mail-Oauth-Token headers",
        ));
    }

    // IMAP host.
    let imap_host = env.get(Field::ImapHost);
    match imap_host {
        Some(_) => r.push(env_set(Field::ImapHost.env(), false)),
        None if http => r.push(per_request(
            Field::ImapHost.env(),
            "X-Mail-Imap-Host header",
        )),
        None => r.push(env_set(Field::ImapHost.env(), false)),
    };

    // Resolve the env source alone: the same code path tool calls use.
    let creds = Credentials::resolve(env, &Settings::default(), true);
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
                    "logged in to {} as {} ({}); INBOX has {n} message(s)",
                    c.imap,
                    c.user,
                    c.auth.kind()
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
            r.push(per_request(
                Field::SmtpHost.env(),
                "X-Mail-Smtp-Host header",
            ));
        }
        None => {
            r.push(Check::pass(
                Field::SmtpHost.env(),
                "not set (not needed: sending is off)",
            ));
        }
    }

    // Mode and send policy.
    let mode = plan.mode.summary(&plan.policy);
    r.push(if plan.mode.sends() && plan.policy.patterns.is_empty() {
        Check::warn("mode", mode)
    } else {
        Check::pass("mode", mode)
    });
    if plan.mode.sends() {
        r.push(Check::pass("send policy", plan.policy.summary()));
    }

    r.push(Check::pass("from policy", plan.from_policy.summary()));
    r.push(Check::pass(
        "sent copy",
        if plan.sent_copy {
            "sent messages are copied to the sent folder (--no-sent-copy to turn off)"
        } else {
            "off (--no-sent-copy)"
        },
    ));
    if http {
        r.push(Check::pass(
            "server endpoints",
            if plan.pin_servers {
                "pinned: IMAP/SMTP servers come from the environment only"
            } else {
                "a caller with its own login may name its servers in X-Mail-Imap-*/X-Mail-Smtp-* \
                 headers (TLS only); --pin-servers to forbid"
            },
        ));
    }

    // Listen address.
    if let Some(bind) = plan.bind {
        r.push(port_free(bind));
    }

    r.push(tool_budget(MailServer::tool_names(plan.mode).len()));
    r
}
