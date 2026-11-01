//! Turning `search` arguments into an IMAP `SEARCH` key string.
//!
//! Every user string becomes an IMAP quoted string: `\` and `"` are escaped, and CR, LF and NUL
//! are refused (a quoted string cannot carry them, and a raw CRLF would end the command and start
//! a new one). Non-ASCII text adds `CHARSET UTF-8` in front of the keys.

use rmcp::schemars;
use serde::Deserialize;

use crate::error::{MailError, Result};

/// Most messages one `search` returns.
pub const MAX_LIMIT: u32 = 100;
/// Default number of messages.
pub const DEFAULT_LIMIT: u32 = 20;

/// The filter part of the `search` tool's arguments.
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub struct Criteria {
    /// Substring of the From header.
    #[serde(default)]
    pub from: Option<String>,
    /// Substring of the To header.
    #[serde(default)]
    pub to: Option<String>,
    /// Substring of the subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// Substring anywhere in the headers or the body.
    #[serde(default)]
    pub text: Option<String>,
    /// Received on or after this date (YYYY-MM-DD).
    #[serde(default)]
    pub since: Option<String>,
    /// Received before this date (YYYY-MM-DD).
    #[serde(default)]
    pub before: Option<String>,
    /// true: only unread messages; false: only read messages.
    #[serde(default)]
    pub unseen: Option<bool>,
}

/// Quote `value` as an IMAP quoted string.
pub fn quote(field: &str, value: &str) -> Result<String> {
    if let Some(bad) = value.chars().find(|c| matches!(c, '\r' | '\n' | '\0')) {
        return Err(MailError::InvalidArgument(format!(
            "{field} must not contain {}",
            match bad {
                '\r' => "a carriage return",
                '\n' => "a line break",
                _ => "a NUL character",
            }
        )));
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Parse `YYYY-MM-DD` into the IMAP date form `D-Mon-YYYY`.
pub fn imap_date(field: &str, value: &str) -> Result<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let bad = || MailError::InvalidArgument(format!("{field} must be a date like 2026-10-31"));
    let mut parts = value.trim().splitn(3, '-');
    let (Some(y), Some(m), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad());
    };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return Err(bad());
    }
    let y: u32 = y.parse().map_err(|_| bad())?;
    let m: usize = m.parse().map_err(|_| bad())?;
    let d: u32 = d.parse().map_err(|_| bad())?;
    let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&m) || d == 0 || d > days[m - 1] {
        return Err(bad());
    }
    Ok(format!("{d}-{}-{y}", MONTHS[m - 1]))
}

/// Build the `UID SEARCH` key string for `c`. No criteria = `ALL`.
pub fn build(c: &Criteria) -> Result<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut non_ascii = false;
    for (key, field, value) in [
        ("FROM", "from", &c.from),
        ("TO", "to", &c.to),
        ("SUBJECT", "subject", &c.subject),
        ("TEXT", "text", &c.text),
    ] {
        if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
            non_ascii |= !v.is_ascii();
            keys.push(format!("{key} {}", quote(field, v)?));
        }
    }
    if let Some(v) = c.since.as_deref().filter(|v| !v.is_empty()) {
        keys.push(format!("SINCE {}", imap_date("since", v)?));
    }
    if let Some(v) = c.before.as_deref().filter(|v| !v.is_empty()) {
        keys.push(format!("BEFORE {}", imap_date("before", v)?));
    }
    match c.unseen {
        Some(true) => keys.push("UNSEEN".into()),
        Some(false) => keys.push("SEEN".into()),
        None => {}
    }
    if keys.is_empty() {
        keys.push("ALL".into());
    }
    let query = keys.join(" ");
    Ok(if non_ascii {
        format!("CHARSET UTF-8 {query}")
    } else {
        query
    })
}

/// Clamp a requested limit to `1..=MAX_LIMIT`.
pub fn clamp_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crit() -> Criteria {
        Criteria::default()
    }

    #[test]
    fn empty_is_all() {
        assert_eq!(build(&crit()).unwrap(), "ALL");
    }

    #[test]
    fn keys_in_order() {
        let c = Criteria {
            from: Some("ann@example.org".into()),
            subject: Some("invoice".into()),
            since: Some("2026-01-05".into()),
            before: Some("2026-12-31".into()),
            unseen: Some(true),
            ..crit()
        };
        assert_eq!(
            build(&c).unwrap(),
            r#"FROM "ann@example.org" SUBJECT "invoice" SINCE 5-Jan-2026 BEFORE 31-Dec-2026 UNSEEN"#
        );
    }

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        // An attempt to close the string and add keys of its own stays one quoted string.
        let c = Criteria {
            subject: Some(r#"x" OR ALL "\"#.into()),
            ..crit()
        };
        let q = build(&c).unwrap();
        assert_eq!(q, r#"SUBJECT "x\" OR ALL \"\\""#);
    }

    #[test]
    fn crlf_injection_is_refused() {
        for evil in [
            "x\r\nA1 DELETE INBOX",
            "x\nA1 LOGOUT",
            "x\rA1 EXPUNGE",
            "nul\0byte",
        ] {
            let c = Criteria {
                text: Some(evil.into()),
                ..crit()
            };
            let err = build(&c).unwrap_err();
            assert!(matches!(err, MailError::InvalidArgument(_)), "{evil:?}");
        }
    }

    #[test]
    fn non_ascii_sets_charset() {
        let c = Criteria {
            from: Some("Zoë".into()),
            ..crit()
        };
        assert_eq!(build(&c).unwrap(), "CHARSET UTF-8 FROM \"Zoë\"");
    }

    #[test]
    fn dates_are_validated() {
        assert_eq!(imap_date("since", "2024-02-29").unwrap(), "29-Feb-2024");
        for bad in [
            "2023-02-29",
            "2026-13-01",
            "2026-1-1",
            "01-01-2026",
            "yesterday",
            "2026-01-01 OR ALL",
        ] {
            assert!(imap_date("since", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn limits_are_clamped() {
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(5000)), MAX_LIMIT);
    }
}
