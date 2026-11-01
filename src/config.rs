//! Credentials and mail-server endpoints.
//!
//! There is one [`Credentials`] struct and two sources for it:
//!
//! * **environment** (`MAIL_USER`, `MAIL_PASSWORD`, `IMAP_HOST`, ...): one mailbox, used by stdio
//!   mode and by a single-user HTTP server;
//! * **forwarded headers** (`X-Mail-User`, `X-Mail-Password`, `X-Imap-Host`, ...): one mailbox per
//!   request on a shared HTTP server (Emperor Profile E6). Only the names in [`HEADER_NAMES`] are
//!   captured; values are used for the one tool call and dropped with the request.
//!
//! Both sources fill the same [`Settings`] and go through [`Credentials::resolve`], so stdio and
//! HTTP run the same code. Rules:
//!
//! * user and password are a pair from one source: if a request carries either header, both must
//!   come from headers; otherwise both come from the environment. A header user name is never
//!   combined with the operator's password.
//! * an endpoint (host, port, security) set in the environment pins the server: the operator
//!   decides where credentials go, and `X-Imap-*` / `X-Smtp-*` headers are then ignored. Headers
//!   fill in only an endpoint the operator left open.
//!
//! There are no default credentials and nothing here is ever written to disk or logged; the
//! password is a [`Secret`] whose `Debug` prints `***`.

use std::fmt;
use std::net::IpAddr;

use emperor_mcp::{ForwardConfig, ForwardedHeaders};

use crate::error::{MailError, Result};

/// One setting, with its environment variable and its forwarded header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Login name.
    User,
    /// Password or app password.
    Password,
    /// `From` address when the login name is not an address.
    From,
    /// IMAP host.
    ImapHost,
    /// IMAP port.
    ImapPort,
    /// IMAP security: `tls`, `starttls` or `plain`.
    ImapSecurity,
    /// SMTP host.
    SmtpHost,
    /// SMTP port.
    SmtpPort,
    /// SMTP security: `tls`, `starttls` or `plain`.
    SmtpSecurity,
}

impl Field {
    /// Every field.
    pub const ALL: [Field; 9] = [
        Field::User,
        Field::Password,
        Field::From,
        Field::ImapHost,
        Field::ImapPort,
        Field::ImapSecurity,
        Field::SmtpHost,
        Field::SmtpPort,
        Field::SmtpSecurity,
    ];

    /// Environment variable name.
    pub fn env(self) -> &'static str {
        match self {
            Field::User => "MAIL_USER",
            Field::Password => "MAIL_PASSWORD",
            Field::From => "MAIL_FROM",
            Field::ImapHost => "IMAP_HOST",
            Field::ImapPort => "IMAP_PORT",
            Field::ImapSecurity => "IMAP_SECURITY",
            Field::SmtpHost => "SMTP_HOST",
            Field::SmtpPort => "SMTP_PORT",
            Field::SmtpSecurity => "SMTP_SECURITY",
        }
    }

    /// Forwarded HTTP header name.
    pub fn header(self) -> &'static str {
        match self {
            Field::User => "X-Mail-User",
            Field::Password => "X-Mail-Password",
            Field::From => "X-Mail-From",
            Field::ImapHost => "X-Imap-Host",
            Field::ImapPort => "X-Imap-Port",
            Field::ImapSecurity => "X-Imap-Security",
            Field::SmtpHost => "X-Smtp-Host",
            Field::SmtpPort => "X-Smtp-Port",
            Field::SmtpSecurity => "X-Smtp-Security",
        }
    }
}

/// The allow-listed forwarded headers, in [`Field::ALL`] order.
pub const HEADER_NAMES: [&str; 9] = [
    "X-Mail-User",
    "X-Mail-Password",
    "X-Mail-From",
    "X-Imap-Host",
    "X-Imap-Port",
    "X-Imap-Security",
    "X-Smtp-Host",
    "X-Smtp-Port",
    "X-Smtp-Security",
];

/// The `ForwardConfig` for `emperor_mcp::serve`: exactly [`HEADER_NAMES`].
pub fn forward_config() -> ForwardConfig {
    ForwardConfig::new(HEADER_NAMES)
}

/// A password. `Debug` never shows it; there is no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for handing to the upstream login only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// How a connection is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// TLS from the first byte (IMAP 993, SMTP 465).
    Tls,
    /// Plain connect, then upgrade with STARTTLS (IMAP 143, SMTP 587). The upgrade is required.
    StartTls,
    /// No encryption. Accepted only for loopback hosts (a local bridge or test server).
    Plain,
}

impl Security {
    /// Parse `tls`, `starttls` or `plain` (case-insensitive).
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "tls" | "ssl" => Ok(Self::Tls),
            "starttls" => Ok(Self::StartTls),
            "plain" | "none" => Ok(Self::Plain),
            other => Err(MailError::Config(format!(
                "security must be tls, starttls or plain, not {other:?}"
            ))),
        }
    }

    /// Name as accepted by [`Self::parse`].
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tls => "tls",
            Self::StartTls => "starttls",
            Self::Plain => "plain",
        }
    }
}

/// Which protocol an endpoint speaks; selects defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// IMAP (reading, drafts).
    Imap,
    /// SMTP (sending).
    Smtp,
}

impl Protocol {
    fn default_port(self, security: Security) -> u16 {
        match (self, security) {
            (Protocol::Imap, Security::Tls) => 993,
            (Protocol::Imap, _) => 143,
            (Protocol::Smtp, Security::Tls) => 465,
            (Protocol::Smtp, _) => 587,
        }
    }

    fn default_security(self, port: u16) -> Security {
        match (self, port) {
            (Protocol::Imap, 143) | (Protocol::Smtp, 587) | (Protocol::Smtp, 25) => {
                Security::StartTls
            }
            _ => Security::Tls,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Protocol::Imap => "IMAP",
            Protocol::Smtp => "SMTP",
        }
    }
}

/// A mail server address with its security mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Host name or IP.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// TLS mode.
    pub security: Security,
}

impl Endpoint {
    /// Build from raw settings, filling port and security from each other.
    fn from_parts(
        protocol: Protocol,
        host: &str,
        port: Option<&str>,
        security: Option<&str>,
    ) -> Result<Self> {
        let host = host.trim();
        if host.is_empty() || host.contains(['/', ' ', '\r', '\n']) {
            return Err(MailError::Config(format!(
                "{} host {host:?} is not a host name",
                protocol.name()
            )));
        }
        let security = security.map(Security::parse).transpose()?;
        let port = port
            .map(|p| {
                p.trim().parse::<u16>().map_err(|_| {
                    MailError::Config(format!("{} port {p:?} is not a port", protocol.name()))
                })
            })
            .transpose()?;
        let (port, security) = match (port, security) {
            (Some(p), Some(s)) => (p, s),
            (Some(p), None) => (p, protocol.default_security(p)),
            (None, Some(s)) => (protocol.default_port(s), s),
            (None, None) => (protocol.default_port(Security::Tls), Security::Tls),
        };
        let endpoint = Self {
            host: host.to_string(),
            port,
            security,
        };
        if security == Security::Plain && !endpoint.is_loopback() {
            return Err(MailError::Config(format!(
                "{} security `plain` is allowed only for a loopback host, not {host}; use tls or starttls",
                protocol.name()
            )));
        }
        Ok(endpoint)
    }

    /// True for `localhost` and loopback IPs.
    pub fn is_loopback(&self) -> bool {
        let h = self.host.trim_start_matches('[').trim_end_matches(']');
        h.eq_ignore_ascii_case("localhost") || h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
    }

    /// `host:port`.
    pub fn target(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{} ({})",
            self.host,
            self.port,
            self.security.as_str()
        )
    }
}

/// Raw settings from one source (environment or forwarded headers). Empty values count as unset.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct Settings {
    values: [Option<String>; 9],
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Settings");
        for field in Field::ALL {
            let shown = match (field, self.get(field)) {
                (_, None) => "unset".to_string(),
                (Field::Password, Some(_)) => "***".to_string(),
                (_, Some(v)) => v.to_string(),
            };
            d.field(field.env(), &shown);
        }
        d.finish()
    }
}

impl Settings {
    /// Build from any lookup (`field -> value`).
    pub fn from_lookup(lookup: impl Fn(Field) -> Option<String>) -> Self {
        let mut values: [Option<String>; 9] = Default::default();
        for (slot, field) in values.iter_mut().zip(Field::ALL) {
            *slot = lookup(field).filter(|v| !v.trim().is_empty());
        }
        Self { values }
    }

    /// From the process environment.
    pub fn from_env() -> Self {
        Self::from_lookup(|f| std::env::var(f.env()).ok())
    }

    /// From the headers `forward_headers_middleware` captured for this request.
    pub fn from_headers(headers: &ForwardedHeaders) -> Self {
        Self::from_lookup(|f| headers.get(f.header()).map(str::to_string))
    }

    /// One value.
    pub fn get(&self, field: Field) -> Option<&str> {
        let i = Field::ALL.iter().position(|f| *f == field)?;
        self.values[i].as_deref()
    }

    /// True when both user and password are set.
    pub fn has_login(&self) -> bool {
        self.get(Field::User).is_some() && self.get(Field::Password).is_some()
    }

    fn endpoint(&self, protocol: Protocol) -> Option<Result<Endpoint>> {
        let (h, p, s) = match protocol {
            Protocol::Imap => (Field::ImapHost, Field::ImapPort, Field::ImapSecurity),
            Protocol::Smtp => (Field::SmtpHost, Field::SmtpPort, Field::SmtpSecurity),
        };
        self.get(h)
            .map(|host| Endpoint::from_parts(protocol, host, self.get(p), self.get(s)))
    }
}

/// Everything one tool call needs to reach one mailbox. Built per call, dropped after it.
#[derive(Debug, Clone)]
pub struct Credentials {
    /// Login name.
    pub user: String,
    /// Password or app password.
    pub password: Secret,
    /// Address used in `From` (defaults to the login name when that is an address).
    pub from: Option<String>,
    /// IMAP server.
    pub imap: Endpoint,
    /// SMTP server, when configured (needed only by `send`).
    pub smtp: Option<Endpoint>,
}

/// Where a resolved login came from (for doctor and logs; never the values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginSource {
    /// Process environment.
    Env,
    /// Forwarded request headers.
    Headers,
}

impl Credentials {
    /// Combine the operator's environment and this request's headers (see the module docs).
    pub fn resolve(env: &Settings, headers: &Settings) -> Result<Self> {
        let header_login =
            headers.get(Field::User).is_some() || headers.get(Field::Password).is_some();
        let (login, source) = if header_login {
            (headers, LoginSource::Headers)
        } else {
            (env, LoginSource::Env)
        };
        let (Some(user), Some(password)) = (login.get(Field::User), login.get(Field::Password))
        else {
            return Err(MailError::MissingCredentials(match source {
                LoginSource::Headers => {
                    "send both X-Mail-User and X-Mail-Password headers".to_string()
                }
                LoginSource::Env => "send X-Mail-User and X-Mail-Password headers, or set \
                                     MAIL_USER and MAIL_PASSWORD for a single mailbox"
                    .to_string(),
            }));
        };

        let imap = match env.endpoint(Protocol::Imap) {
            Some(pinned) => pinned?,
            None => headers
                .endpoint(Protocol::Imap)
                .transpose()?
                .ok_or_else(|| {
                    MailError::MissingCredentials(
                        "no IMAP host: set IMAP_HOST or send an X-Imap-Host header".to_string(),
                    )
                })?,
        };
        let smtp = match env.endpoint(Protocol::Smtp) {
            Some(pinned) => Some(pinned?),
            None => headers.endpoint(Protocol::Smtp).transpose()?,
        };
        let from = login
            .get(Field::From)
            .map(str::to_string)
            .or_else(|| user.contains('@').then(|| user.to_string()));

        Ok(Self {
            user: user.to_string(),
            password: Secret::new(password),
            from,
            imap,
            smtp,
        })
    }

    /// The SMTP endpoint, or a clear error.
    pub fn smtp(&self) -> Result<&Endpoint> {
        self.smtp.as_ref().ok_or_else(|| {
            MailError::MissingCredentials(
                "no SMTP host: set SMTP_HOST or send an X-Smtp-Host header".to_string(),
            )
        })
    }

    /// The `From` address, or a clear error.
    pub fn from_address(&self) -> Result<&str> {
        self.from.as_deref().ok_or_else(|| {
            MailError::MissingCredentials(
                "the login name is not an email address: set MAIL_FROM or send X-Mail-From"
                    .to_string(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(Field, &str)]) -> Settings {
        Settings::from_lookup(|f| {
            pairs
                .iter()
                .find(|(k, _)| *k == f)
                .map(|(_, v)| v.to_string())
        })
    }

    #[test]
    fn header_names_match_fields() {
        let from_fields: Vec<&str> = Field::ALL.iter().map(|f| f.header()).collect();
        assert_eq!(from_fields, HEADER_NAMES);
    }

    #[test]
    fn env_source_alone() {
        let env = settings(&[
            (Field::User, "ann@example.org"),
            (Field::Password, "pw"),
            (Field::ImapHost, "imap.example.org"),
        ]);
        let c = Credentials::resolve(&env, &Settings::default()).unwrap();
        assert_eq!(c.user, "ann@example.org");
        assert_eq!(c.imap.port, 993);
        assert_eq!(c.imap.security, Security::Tls);
        assert_eq!(c.from.as_deref(), Some("ann@example.org"));
        assert!(c.smtp.is_none());
    }

    #[test]
    fn header_login_never_mixes_with_env_password() {
        let env = settings(&[
            (Field::User, "operator"),
            (Field::Password, "operator-pw"),
            (Field::ImapHost, "imap.example.org"),
        ]);
        let headers = settings(&[(Field::User, "mallory")]);
        let err = Credentials::resolve(&env, &headers).unwrap_err();
        assert!(matches!(err, MailError::MissingCredentials(_)), "{err}");
        assert!(err.to_string().contains("X-Mail-Password"));
    }

    #[test]
    fn env_endpoint_pins_the_host() {
        let env = settings(&[(Field::ImapHost, "imap.corp.example")]);
        let headers = settings(&[
            (Field::User, "bob"),
            (Field::Password, "pw"),
            (Field::ImapHost, "evil.example"),
        ]);
        let c = Credentials::resolve(&env, &headers).unwrap();
        assert_eq!(c.imap.host, "imap.corp.example");
    }

    #[test]
    fn headers_fill_an_open_endpoint() {
        let headers = settings(&[
            (Field::User, "bob@example.net"),
            (Field::Password, "pw"),
            (Field::ImapHost, "imap.example.net"),
            (Field::ImapPort, "143"),
            (Field::SmtpHost, "smtp.example.net"),
        ]);
        let c = Credentials::resolve(&Settings::default(), &headers).unwrap();
        assert_eq!(c.imap.security, Security::StartTls);
        assert_eq!(c.smtp().unwrap().port, 465);
    }

    #[test]
    fn nothing_set_is_a_clear_error() {
        let err = Credentials::resolve(&Settings::default(), &Settings::default()).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("X-Mail-User") && text.contains("MAIL_USER"),
            "{text}"
        );
    }

    #[test]
    fn plain_only_on_loopback() {
        let ok = settings(&[
            (Field::User, "u"),
            (Field::Password, "p"),
            (Field::ImapHost, "127.0.0.1"),
            (Field::ImapPort, "3143"),
            (Field::ImapSecurity, "plain"),
        ]);
        assert!(Credentials::resolve(&ok, &Settings::default()).is_ok());
        let bad = settings(&[
            (Field::User, "u"),
            (Field::Password, "p"),
            (Field::ImapHost, "imap.example.org"),
            (Field::ImapSecurity, "plain"),
        ]);
        let err = Credentials::resolve(&bad, &Settings::default()).unwrap_err();
        assert!(err.to_string().contains("loopback"), "{err}");
    }

    #[test]
    fn password_never_in_debug() {
        let env = settings(&[
            (Field::User, "u"),
            (Field::Password, "hunter2"),
            (Field::ImapHost, "h"),
        ]);
        let c = Credentials::resolve(&env, &Settings::default()).unwrap();
        assert!(!format!("{c:?}").contains("hunter2"));
        assert!(!format!("{env:?}").contains("hunter2"));
    }

    #[test]
    fn bad_port_is_a_config_error() {
        let env = settings(&[
            (Field::User, "u"),
            (Field::Password, "p"),
            (Field::ImapHost, "h"),
            (Field::ImapPort, "99999"),
        ]);
        assert!(matches!(
            Credentials::resolve(&env, &Settings::default()),
            Err(MailError::Config(_))
        ));
    }
}
