//! Who `send` may write to.
//!
//! Sending is off unless the server was started with `--allow-send`, and then only to recipients
//! that match a `--send-allow` pattern: an exact address (`ann@example.org`) or a whole domain
//! (`@example.org`, no subdomains). Every recipient (To and Cc) must match. `draft` is never
//! restricted: a draft waits in the Drafts folder for a human.

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
    /// Parse `user@domain` or `@domain`.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim().to_ascii_lowercase();
        let invalid = || {
            MailError::Config(format!(
                "send-allow pattern {s:?} must be an address (ann@example.org) or a domain (@example.org)"
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
                "sending is disabled on this server (it was not started with --allow-send). \
                 Use the `draft` tool instead: it saves the message in the Drafts folder for a \
                 person to review and send."
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
            self.patterns
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    /// One line for doctor and the start-up log.
    pub fn summary(&self) -> String {
        if !self.enabled {
            "send off: draft only".to_string()
        } else if self.patterns.is_empty() {
            "send on, but no --send-allow pattern: every recipient is refused".to_string()
        } else {
            format!("send on, to {}", self.describe_patterns())
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
    fn disabled_suggests_draft() {
        let err = SendPolicy::default().check(["a@example.org"]).unwrap_err();
        assert!(err.to_string().contains("--allow-send"));
        assert!(err.to_string().contains("`draft`"));
    }

    #[test]
    fn enabled_without_patterns_refuses() {
        let p = SendPolicy::new(true, &[]).unwrap();
        assert!(p.check(["a@example.org"]).is_err());
        assert!(p.summary().contains("every recipient is refused"));
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
}
