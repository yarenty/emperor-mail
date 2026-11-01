//! What the server may write, and who `send` may write to.
//!
//! [`Mode`] is fixed at start-up. By default the server only reads: `draft` and `send` are not
//! listed. `--writable` (`MAIL_WRITABLE`) adds `draft`; `--allow-send` (`MAIL_ALLOW_SEND`) adds
//! `send` and implies `--writable`.
//!
//! With sending on, `send` writes only to recipients that match a `--send-allow`
//! (`MAIL_SEND_ALLOW`) pattern: an exact address (`ann@example.org`) or a whole domain
//! (`@example.org`, no subdomains). Every recipient (To and Cc) must match; no pattern refuses
//! every recipient. `draft` is never restricted by recipient: a draft waits in the Drafts folder
//! for a human.
//!
//! [`FromPolicy`] (`--from-allow`, same pattern syntax) limits the `From` address of both `send`
//! and `draft`, so a caller on a shared server cannot write as someone else through
//! `X-Mail-From`.

use crate::error::{MailError, Result};

/// One allowed recipient pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// Exactly this address (case-insensitive).
    Address(String),
    /// Any address at exactly this domain (case-insensitive).
    Domain(String),
}

impl Pattern {
    /// Parse `user@domain` or `@domain` (a `--send-allow` pattern).
    pub fn parse(s: &str) -> Result<Self> {
        Self::parse_for("send-allow", s)
    }

    /// Parse `user@domain` or `@domain`; `flag` names the option in the error.
    pub fn parse_for(flag: &str, s: &str) -> Result<Self> {
        let s = s.trim().to_ascii_lowercase();
        let invalid = || {
            MailError::Config(format!(
                "{flag} pattern {s:?} must be an address (ann@example.org) or a domain (@example.org)"
            ))
        };
        let (local, domain) = s.split_once('@').ok_or_else(invalid)?;
        if domain.is_empty()
            || domain.contains('@')
            || domain.starts_with('.')
            || domain.ends_with('.')
            || !domain.contains('.') && domain != "localhost"
            || s.contains(char::is_whitespace)
        {
            return Err(invalid());
        }
        if local.is_empty() {
            Ok(Self::Domain(domain.to_string()))
        } else {
            Ok(Self::Address(s))
        }
    }

    /// Does `address` (a bare address, no display name) match?
    pub fn matches(&self, address: &str) -> bool {
        let address = address.trim().to_ascii_lowercase();
        match self {
            Self::Address(a) => *a == address,
            Self::Domain(d) => address
                .rsplit_once('@')
                .is_some_and(|(local, domain)| !local.is_empty() && domain == d),
        }
    }
}

impl std::fmt::Display for Pattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Address(a) => f.write_str(a),
            Self::Domain(d) => write!(f, "@{d}"),
        }
    }
}

/// What the server may write, fixed at start-up. It decides which tools are listed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// No switch: the four reading tools only.
    #[default]
    ReadOnly,
    /// `--writable`: adds `draft`.
    Drafts,
    /// `--allow-send`: adds `draft` and `send`.
    DraftsAndSend,
}

impl Mode {
    /// The mode for the start-up switches: sending on implies writable.
    pub fn new(writable: bool, policy: &SendPolicy) -> Self {
        if policy.enabled {
            Self::DraftsAndSend
        } else if writable {
            Self::Drafts
        } else {
            Self::ReadOnly
        }
    }

    /// `draft` is listed.
    pub fn drafts(self) -> bool {
        self != Self::ReadOnly
    }

    /// `send` is listed.
    pub fn sends(self) -> bool {
        self == Self::DraftsAndSend
    }

    /// One line for doctor and the start-up log.
    pub fn summary(self, policy: &SendPolicy) -> String {
        match self {
            Self::ReadOnly => {
                "read-only: draft and send are not listed (--writable adds draft, --allow-send \
                 adds send)"
                    .to_string()
            }
            Self::Drafts => {
                "drafts: draft is listed, send is not (--allow-send adds it)".to_string()
            }
            Self::DraftsAndSend => format!(
                "drafts+send with {} recipient pattern(s){}",
                policy.patterns.len(),
                if policy.patterns.is_empty() {
                    ": every recipient is refused"
                } else {
                    ""
                }
            ),
        }
    }
}

/// The server's send policy, fixed at start-up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendPolicy {
    /// `--allow-send` was given.
    pub enabled: bool,
    /// `--send-allow` patterns.
    pub patterns: Vec<Pattern>,
}

impl SendPolicy {
    /// Build from the CLI flags.
    pub fn new(enabled: bool, patterns: &[String]) -> Result<Self> {
        Ok(Self {
            enabled,
            patterns: patterns
                .iter()
                .map(|p| Pattern::parse(p))
                .collect::<Result<_>>()?,
        })
    }

    /// Refuse unless sending is on and every recipient matches a pattern.
    pub fn check<'a>(&self, recipients: impl IntoIterator<Item = &'a str>) -> Result<()> {
        if !self.enabled {
            return Err(MailError::SendRefused(
                "sending is disabled on this server (it was not started with --allow-send or \
                 MAIL_ALLOW_SEND)"
                    .to_string(),
            ));
        }
        let refused: Vec<&str> = recipients
            .into_iter()
            .filter(|r| !self.patterns.iter().any(|p| p.matches(r)))
            .collect();
        if refused.is_empty() {
            Ok(())
        } else {
            Err(MailError::SendRefused(format!(
                "{} not in the allowed recipients ({}). Use the `draft` tool instead: it saves \
                 the message in the Drafts folder for a person to review and send.",
                refused.join(", "),
                self.describe_patterns()
            )))
        }
    }

    fn describe_patterns(&self) -> String {
        if self.patterns.is_empty() {
            "none configured".to_string()
        } else {
            join(&self.patterns)
        }
    }

    /// One line for doctor and the start-up log.
    pub fn summary(&self) -> String {
        if !self.enabled {
            "send off".to_string()
        } else if self.patterns.is_empty() {
            "send on, 0 recipient patterns: every recipient is refused".to_string()
        } else {
            format!(
                "send on, {} recipient pattern(s): {}",
                self.patterns.len(),
                self.describe_patterns()
            )
        }
    }
}

fn join(patterns: &[Pattern]) -> String {
    patterns
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Which `From` addresses `send` and `draft` may use (`--from-allow`). Empty: any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FromPolicy {
    /// `--from-allow` patterns.
    pub patterns: Vec<Pattern>,
}

impl FromPolicy {
    /// Build from the CLI flag.
    pub fn new(patterns: &[String]) -> Result<Self> {
        Ok(Self {
            patterns: patterns
                .iter()
                .map(|p| Pattern::parse_for("from-allow", p))
                .collect::<Result<_>>()?,
        })
    }

    /// Refuse a `From` address that matches no pattern (when there are patterns).
    pub fn check(&self, from: &str) -> Result<()> {
        let address = from
            .rsplit_once('<')
            .map(|(_, rest)| rest.trim_end().trim_end_matches('>'))
            .unwrap_or(from);
        if self.patterns.is_empty() || self.patterns.iter().any(|p| p.matches(address)) {
            Ok(())
        } else {
            Err(MailError::FromRefused(format!(
                "{address} is not an allowed From address on this server ({}); set X-Mail-From \
                 or MAIL_FROM to an allowed address",
                join(&self.patterns)
            )))
        }
    }

    /// One line for doctor and the start-up log.
    pub fn summary(&self) -> String {
        if self.patterns.is_empty() {
            "any From address".to_string()
        } else {
            format!("From limited to {}", join(&self.patterns))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_domain_patterns() {
        let p = SendPolicy::new(true, &["Ann@Example.org".into(), "@team.example".into()]).unwrap();
        assert!(p.check(["ann@example.org"]).is_ok());
        assert!(p.check(["ANN@EXAMPLE.ORG", "zed@team.example"]).is_ok());
        assert!(p.check(["bob@example.org"]).is_err());
        // Domain patterns do not cover subdomains or look-alikes.
        assert!(p.check(["x@sub.team.example"]).is_err());
        assert!(p.check(["x@team.example.evil"]).is_err());
        assert!(p.check(["@team.example"]).is_err());
    }

    #[test]
    fn one_bad_recipient_refuses_all() {
        let p = SendPolicy::new(true, &["@example.org".into()]).unwrap();
        let err = p.check(["a@example.org", "b@elsewhere.net"]).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("b@elsewhere.net") && text.contains("draft"),
            "{text}"
        );
    }

    #[test]
    fn disabled_names_the_switch() {
        let err = SendPolicy::default().check(["a@example.org"]).unwrap_err();
        assert!(err.to_string().contains("--allow-send"));
    }

    #[test]
    fn modes() {
        let off = SendPolicy::default();
        let on = SendPolicy::new(true, &["@a.example".into()]).unwrap();
        assert_eq!(Mode::new(false, &off), Mode::ReadOnly);
        assert_eq!(Mode::new(true, &off), Mode::Drafts);
        // Sending implies drafts, with or without --writable.
        assert_eq!(Mode::new(false, &on), Mode::DraftsAndSend);
        assert_eq!(Mode::new(true, &on), Mode::DraftsAndSend);
        assert!(!Mode::ReadOnly.drafts() && !Mode::ReadOnly.sends());
        assert!(Mode::Drafts.drafts() && !Mode::Drafts.sends());
        assert!(Mode::DraftsAndSend.drafts() && Mode::DraftsAndSend.sends());
        assert!(Mode::ReadOnly.summary(&off).starts_with("read-only"));
        assert!(Mode::Drafts.summary(&off).starts_with("drafts:"));
        assert_eq!(
            Mode::DraftsAndSend.summary(&on),
            "drafts+send with 1 recipient pattern(s)"
        );
        let none = SendPolicy::new(true, &[]).unwrap();
        assert!(
            Mode::DraftsAndSend
                .summary(&none)
                .ends_with("every recipient is refused")
        );
    }

    #[test]
    fn enabled_without_patterns_refuses() {
        let p = SendPolicy::new(true, &[]).unwrap();
        assert!(p.check(["a@example.org"]).is_err());
        assert!(p.summary().contains("every recipient is refused"));
        assert!(p.summary().contains("0 recipient patterns"));
        let p = SendPolicy::new(true, &["@a.example".into(), "b@c.example".into()]).unwrap();
        assert!(p.summary().starts_with("send on, 2 recipient pattern(s)"));
        assert_eq!(SendPolicy::default().summary(), "send off");
    }

    #[test]
    fn bad_patterns_are_rejected() {
        for bad in [
            "example.org",
            "@",
            "a@",
            "@nodot",
            "a@b@c.org",
            "@.org",
            "a b@c.org",
        ] {
            assert!(Pattern::parse(bad).is_err(), "{bad} should be rejected");
        }
        assert_eq!(
            Pattern::parse("@localhost").unwrap(),
            Pattern::Domain("localhost".into())
        );
    }

    #[test]
    fn from_policy() {
        assert!(FromPolicy::default().check("anyone@example.net").is_ok());
        let p = FromPolicy::new(&["@corp.example".into(), "ceo@other.example".into()]).unwrap();
        assert!(p.check("ann@corp.example").is_ok());
        assert!(p.check("Ann <ANN@corp.example>").is_ok());
        assert!(p.check("ceo@other.example").is_ok());
        let err = p.check("eve@evil.example").unwrap_err();
        assert!(matches!(err, MailError::FromRefused(_)), "{err}");
        assert!(p.check("x@sub.corp.example").is_err());
        let err = FromPolicy::new(&["corp.example".into()]).unwrap_err();
        assert!(err.to_string().contains("from-allow"), "{err}");
    }
}
