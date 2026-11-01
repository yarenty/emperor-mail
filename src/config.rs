//! Credentials and mail-server endpoints.
//!
//! There is one [`Credentials`] struct and two sources for it:
//!
//! * **environment** (`MAIL_USER`, `MAIL_PASSWORD` or `MAIL_OAUTH_TOKEN`, `IMAP_HOST`, ...): one
//!   mailbox, used by stdio mode and by a single-user HTTP server;
//! * **forwarded headers** (`X-Mail-User`, `X-Mail-Password` or `X-Mail-Oauth-Token`,
//!   `X-Mail-Imap-Host`, ...): one mailbox per request on a shared HTTP server (Emperor Profile
//!   E6). Only the names in [`HEADER_NAMES`] are captured; values are used for the one tool call
//!   and dropped with the request.
//!
//! Both sources fill the same [`Settings`] and go through [`Credentials::resolve`], so stdio and
//! HTTP run the same code. Rules:
//!
//! * the login is one source: if a request carries any login header (user, password or OAuth
//!   token), the whole login comes from headers; otherwise it all comes from the environment. A
//!   header user name is never combined with the operator's password or token. A login is a user
//!   plus exactly one of password and OAuth access token.
//! * endpoints follow the login. A caller who brings their own login may name their own servers
//!   (`X-Mail-Imap-*` / `X-Mail-Smtp-*`); an endpoint the request leaves out falls back to the
//!   environment. With an environment login, header endpoints are ignored: the operator's
//!   password never goes to a host a caller picked. `--pin-servers` makes the environment
//!   endpoints win for every caller.
//! * a host named in a header is always reached over TLS or STARTTLS; `plain` is accepted only
//!   from the environment, and only for a loopback host.
//!
//! There are no default credentials and nothing here is ever written to disk or logged; the
//! password and the token are [`Secret`]s whose `Debug` prints `***`.

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
    /// OAuth 2.0 access token (XOAUTH2), instead of a password.
    OauthToken,
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
    pub const ALL: [Field; 10] = [
        Field::User,
        Field::Password,
        Field::OauthToken,
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
            Field::OauthToken => "MAIL_OAUTH_TOKEN",
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
            Field::OauthToken => "X-Mail-Oauth-Token",
            Field::From => "X-Mail-From",
            Field::ImapHost => "X-Mail-Imap-Host",
            Field::ImapPort => "X-Mail-Imap-Port",
            Field::ImapSecurity => "X-Mail-Imap-Security",
            Field::SmtpHost => "X-Mail-Smtp-Host",
            Field::SmtpPort => "X-Mail-Smtp-Port",
            Field::SmtpSecurity => "X-Mail-Smtp-Security",
        }
    }

    /// True for values that must never be shown (password, token).
    pub fn is_secret(self) -> bool {
        matches!(self, Field::Password | Field::OauthToken)
    }
}

/// The allow-listed forwarded headers, in [`Field::ALL`] order.
pub const HEADER_NAMES: [&str; 10] = [
    "X-Mail-User",
    "X-Mail-Password",
    "X-Mail-Oauth-Token",
    "X-Mail-From",
    "X-Mail-Imap-Host",
    "X-Mail-Imap-Port",
    "X-Mail-Imap-Security",
    "X-Mail-Smtp-Host",
    "X-Mail-Smtp-Port",
    "X-Mail-Smtp-Security",
];

/// The `ForwardConfig` for `emperor_mcp::serve`: exactly [`HEADER_NAMES`].
pub fn forward_config() -> ForwardConfig {
    ForwardConfig::new(HEADER_NAMES)
}

/// A password or access token. `Debug` never shows it; there is no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Length in characters (for doctor).
    pub fn len(&self) -> usize {
        self.0.chars().count()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
        source: LoginSource,
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
        if security == Security::Plain && source == LoginSource::Headers {
            return Err(MailError::Config(format!(
                "{} security `plain` is not accepted from request headers: a server named in a \
                 request is reached over tls or starttls only",
                protocol.name()
            )));
        }
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
    values: [Option<String>; 10],
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Settings");
        for field in Field::ALL {
            let shown = match (field, self.get(field)) {
                (_, None) => "unset".to_string(),
                (f, Some(_)) if f.is_secret() => "***".to_string(),
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
        let mut values: [Option<String>; 10] = Default::default();
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

    /// True when a user and a password or token are set.
    pub fn has_login(&self) -> bool {
        self.get(Field::User).is_some()
            && (self.get(Field::Password).is_some() || self.get(Field::OauthToken).is_some())
    }

    /// True when any login field (user, password, token) is set.
    fn any_login(&self) -> bool {
        [Field::User, Field::Password, Field::OauthToken]
            .iter()
            .any(|f| self.get(*f).is_some())
    }

    fn endpoint(&self, protocol: Protocol, source: LoginSource) -> Option<Result<Endpoint>> {
        let (h, p, s) = match protocol {
            Protocol::Imap => (Field::ImapHost, Field::ImapPort, Field::ImapSecurity),
            Protocol::Smtp => (Field::SmtpHost, Field::SmtpPort, Field::SmtpSecurity),
        };
        self.get(h)
            .map(|host| Endpoint::from_parts(protocol, host, self.get(p), self.get(s), source))
    }
}

/// How the login proves itself to the mail servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// Password or app password (IMAP `LOGIN`, SMTP `PLAIN`/`LOGIN`).
    Password(Secret),
    /// OAuth 2.0 access token (SASL `XOAUTH2` on IMAP and SMTP). The client obtains and refreshes
    /// it; the server only forwards it.
    OAuth(Secret),
}

impl Auth {
    /// The secret, for the upstream login only.
    pub fn secret(&self) -> &Secret {
        match self {
            Auth::Password(s) | Auth::OAuth(s) => s,
        }
    }

    /// `password` or `oauth`, for doctor and logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Auth::Password(_) => "password",
            Auth::OAuth(_) => "oauth",
        }
    }
}

/// The SASL `XOAUTH2` initial response (before base64): `user=<user>^Aauth=Bearer <token>^A^A`.
/// Used for both IMAP `AUTHENTICATE XOAUTH2` and, through lettre, SMTP `AUTH XOAUTH2`.
pub fn xoauth2_initial_response(user: &str, token: &Secret) -> String {
    format!("user={user}\x01auth=Bearer {}\x01\x01", token.expose())
}

/// Everything one tool call needs to reach one mailbox. Built per call, dropped after it.
#[derive(Debug, Clone)]
pub struct Credentials {
    /// Login name.
    pub user: String,
    /// Password or OAuth access token.
    pub auth: Auth,
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
    /// With `pin_servers`, endpoints always come from the environment.
    pub fn resolve(env: &Settings, headers: &Settings, pin_servers: bool) -> Result<Self> {
        let (login, source) = if headers.any_login() {
            (headers, LoginSource::Headers)
        } else {
            (env, LoginSource::Env)
        };
        let (user_name, password_name, token_name) = match source {
            LoginSource::Headers => ("X-Mail-User", "X-Mail-Password", "X-Mail-Oauth-Token"),
            LoginSource::Env => ("MAIL_USER", "MAIL_PASSWORD", "MAIL_OAUTH_TOKEN"),
        };
        let user = login.get(Field::User);
        let auth = match (login.get(Field::Password), login.get(Field::OauthToken)) {
            (Some(_), Some(_)) => {
                return Err(MailError::Config(format!(
                    "set {password_name} or {token_name}, not both"
                )));
            }
            (Some(p), None) => Some(Auth::Password(Secret::new(p))),
            (None, Some(t)) => Some(Auth::OAuth(Secret::new(t))),
            (None, None) => None,
        };
        let (Some(user), Some(auth)) = (user, auth) else {
            return Err(MailError::MissingCredentials(match source {
                LoginSource::Headers => format!(
                    "send {user_name} with {password_name} (or {token_name}) in the same request"
                ),
                LoginSource::Env => "send X-Mail-User and X-Mail-Password (or X-Mail-Oauth-Token) \
                                     headers, or set MAIL_USER and MAIL_PASSWORD (or \
                                     MAIL_OAUTH_TOKEN) for a single mailbox"
                    .to_string(),
            }));
        };

        // Header endpoints only for a header login, and never when the operator pinned servers.
        let header_endpoints = source == LoginSource::Headers && !pin_servers;
        let pick = |protocol: Protocol| -> Result<Option<Endpoint>> {
            if header_endpoints && let Some(ep) = headers.endpoint(protocol, LoginSource::Headers) {
                return ep.map(Some);
            }
            env.endpoint(protocol, LoginSource::Env).transpose()
        };
        let imap = pick(Protocol::Imap)?.ok_or_else(|| {
            MailError::MissingCredentials(if header_endpoints {
                "no IMAP host: set IMAP_HOST or send an X-Mail-Imap-Host header".to_string()
            } else {
                "no IMAP host: set IMAP_HOST".to_string()
            })
        })?;
        let smtp = pick(Protocol::Smtp)?;
        let from = login
            .get(Field::From)
            .map(str::to_string)
            .or_else(|| user.contains('@').then(|| user.to_string()));

        Ok(Self {
            user: user.to_string(),
            auth,
            from,
            imap,
            smtp,
        })
    }

    /// The SMTP endpoint, or a clear error.
    pub fn smtp(&self) -> Result<&Endpoint> {
        self.smtp.as_ref().ok_or_else(|| {
            MailError::MissingCredentials(
                "no SMTP host: set SMTP_HOST or send an X-Mail-Smtp-Host header".to_string(),
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

    fn resolve(env: &Settings, headers: &Settings) -> Result<Credentials> {
        Credentials::resolve(env, headers, false)
    }

    #[test]
    fn header_names_match_fields() {
        let from_fields: Vec<&str> = Field::ALL.iter().map(|f| f.header()).collect();
        assert_eq!(from_fields, HEADER_NAMES);
        assert!(HEADER_NAMES.iter().all(|h| h.starts_with("X-Mail-")));
    }

    #[test]
    fn env_source_alone() {
        let env = settings(&[
            (Field::User, "ann@example.org"),
            (Field::Password, "pw"),
            (Field::ImapHost, "imap.example.org"),
        ]);
        let c = resolve(&env, &Settings::default()).unwrap();
        assert_eq!(c.user, "ann@example.org");
        assert_eq!(c.imap.port, 993);
        assert_eq!(c.imap.security, Security::Tls);
        assert_eq!(c.from.as_deref(), Some("ann@example.org"));
        assert_eq!(c.auth.kind(), "password");
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
        let err = resolve(&env, &headers).unwrap_err();
        assert!(matches!(err, MailError::MissingCredentials(_)), "{err}");
        assert!(err.to_string().contains("X-Mail-Password"));
        // A token header alone does not borrow the operator's user either.
        let headers = settings(&[(Field::OauthToken, "tok")]);
        assert!(resolve(&env, &headers).is_err());
    }

    #[test]
    fn header_endpoint_wins_for_a_header_login() {
        let env = settings(&[(Field::ImapHost, "imap.corp.example")]);
        let headers = settings(&[
            (Field::User, "bob"),
            (Field::Password, "pw"),
            (Field::ImapHost, "imap.bob.example"),
        ]);
        let c = resolve(&env, &headers).unwrap();
        assert_eq!(c.imap.host, "imap.bob.example");
    }

    #[test]
    fn env_endpoint_is_the_fallback() {
        let env = settings(&[
            (Field::ImapHost, "imap.corp.example"),
            (Field::SmtpHost, "smtp.corp.example"),
        ]);
        let headers = settings(&[
            (Field::User, "bob"),
            (Field::Password, "pw"),
            (Field::SmtpHost, "smtp.bob.example"),
        ]);
        let c = resolve(&env, &headers).unwrap();
        assert_eq!(c.imap.host, "imap.corp.example");
        assert_eq!(c.smtp().unwrap().host, "smtp.bob.example");
    }

    #[test]
    fn pinned_servers_ignore_header_endpoints() {
        let env = settings(&[(Field::ImapHost, "imap.corp.example")]);
        let headers = settings(&[
            (Field::User, "bob"),
            (Field::Password, "pw"),
            (Field::ImapHost, "evil.example"),
        ]);
        let c = Credentials::resolve(&env, &headers, true).unwrap();
        assert_eq!(c.imap.host, "imap.corp.example");
    }

    #[test]
    fn env_login_never_goes_to_a_header_host() {
        let env = settings(&[
            (Field::User, "operator@corp.example"),
            (Field::Password, "operator-pw"),
            (Field::ImapHost, "imap.corp.example"),
        ]);
        let headers = settings(&[(Field::ImapHost, "evil.example")]);
        let c = resolve(&env, &headers).unwrap();
        assert_eq!(c.imap.host, "imap.corp.example");
        // And without an operator host there is no endpoint at all.
        let env = settings(&[(Field::User, "operator"), (Field::Password, "pw")]);
        let err = resolve(&env, &headers).unwrap_err();
        assert!(err.to_string().contains("IMAP_HOST"), "{err}");
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
        let c = resolve(&Settings::default(), &headers).unwrap();
        assert_eq!(c.imap.security, Security::StartTls);
        assert_eq!(c.smtp().unwrap().port, 465);
    }

    #[test]
    fn header_hosts_are_never_plain() {
        for host in ["127.0.0.1", "imap.example.net"] {
            let headers = settings(&[
                (Field::User, "bob"),
                (Field::Password, "pw"),
                (Field::ImapHost, host),
                (Field::ImapSecurity, "plain"),
            ]);
            let err = resolve(&Settings::default(), &headers).unwrap_err();
            assert!(err.to_string().contains("tls or starttls only"), "{err}");
        }
    }

    #[test]
    fn nothing_set_is_a_clear_error() {
        let err = resolve(&Settings::default(), &Settings::default()).unwrap_err();
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
        assert!(resolve(&ok, &Settings::default()).is_ok());
        let bad = settings(&[
            (Field::User, "u"),
            (Field::Password, "p"),
            (Field::ImapHost, "imap.example.org"),
            (Field::ImapSecurity, "plain"),
        ]);
        let err = resolve(&bad, &Settings::default()).unwrap_err();
        assert!(err.to_string().contains("loopback"), "{err}");
    }

    #[test]
    fn secrets_never_in_debug() {
        let env = settings(&[
            (Field::User, "u"),
            (Field::Password, "hunter2"),
            (Field::ImapHost, "h"),
        ]);
        let c = resolve(&env, &Settings::default()).unwrap();
        assert!(!format!("{c:?}").contains("hunter2"));
        assert!(!format!("{env:?}").contains("hunter2"));
        let env = settings(&[
            (Field::User, "u"),
            (Field::OauthToken, "ya29.fake-token"),
            (Field::ImapHost, "h"),
        ]);
        let c = resolve(&env, &Settings::default()).unwrap();
        assert!(!format!("{c:?}").contains("fake-token"));
        assert!(!format!("{env:?}").contains("fake-token"));
    }

    #[test]
    fn oauth_token_is_a_login() {
        let headers = settings(&[
            (Field::User, "ann@example.org"),
            (Field::OauthToken, "fake-access-token"),
            (Field::ImapHost, "imap.example.org"),
        ]);
        let c = resolve(&Settings::default(), &headers).unwrap();
        assert_eq!(c.auth, Auth::OAuth(Secret::new("fake-access-token")));
        let both = settings(&[
            (Field::User, "ann@example.org"),
            (Field::Password, "pw"),
            (Field::OauthToken, "tok"),
            (Field::ImapHost, "imap.example.org"),
        ]);
        let err = resolve(&Settings::default(), &both).unwrap_err();
        assert!(err.to_string().contains("not both"), "{err}");
    }

    #[test]
    fn xoauth2_sasl_string() {
        let s = xoauth2_initial_response("ann@example.org", &Secret::new("fake-token-123"));
        assert_eq!(
            s,
            "user=ann@example.org\x01auth=Bearer fake-token-123\x01\x01"
        );
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
            resolve(&env, &Settings::default()),
            Err(MailError::Config(_))
        ));
    }
}
