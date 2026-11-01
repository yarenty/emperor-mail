//! Building outgoing messages (drafts and sends) with lettre.
//!
//! Every message carries `X-Mailer: emperor-mail/<version>` so a recipient or an auditor can tell
//! which messages an agent wrote. The body is Markdown and goes out as `multipart/alternative`:
//! `text/plain` with the Markdown as written (readable as it is) and `text/html` rendered from it.
//!
//! The HTML rendering takes no raw HTML from the body: HTML blocks and inline tags are escaped
//! and shown as text. Images become plain links, so a message never makes the recipient's mail
//! client fetch a URL on opening (a model steered by injected text could otherwise hide data in an
//! image URL).

use lettre::message::header::{Header, HeaderName, HeaderValue};
use lettre::message::{Mailbox, Message, MultiPart};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::error::{MailError, Result};

/// The `X-Mailer` value.
pub const X_MAILER: &str = concat!("emperor-mail/", env!("CARGO_PKG_VERSION"));

/// `X-Mailer` header.
#[derive(Debug, Clone)]
struct XMailer(String);

impl Header for XMailer {
    fn name() -> HeaderName {
        HeaderName::new_from_ascii_str("X-Mailer")
    }

    fn parse(s: &str) -> std::result::Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self(s.to_string()))
    }

    fn display(&self) -> HeaderValue {
        HeaderValue::new(Self::name(), self.0.clone())
    }
}

/// What the original message says, for a reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplyTo {
    /// Message-ID of the original (without angle brackets).
    pub message_id: Option<String>,
    /// References of the original.
    pub references: Vec<String>,
    /// Subject of the original.
    pub subject: Option<String>,
    /// Where a reply goes when no `to` was given.
    pub address: Option<String>,
}

/// A message to draft or send.
#[derive(Debug, Clone, Default)]
pub struct Outgoing {
    /// From address.
    pub from: String,
    /// To recipients (`addr` or `Name <addr>`).
    pub to: Vec<String>,
    /// Cc recipients.
    pub cc: Vec<String>,
    /// Subject (for a reply, defaults to `Re: <original>`).
    pub subject: Option<String>,
    /// Markdown body (plain text is valid Markdown).
    pub body: String,
    /// Set when replying.
    pub reply: Option<ReplyTo>,
}

fn mailbox(field: &str, s: &str) -> Result<Mailbox> {
    s.trim()
        .parse::<Mailbox>()
        .map_err(|e| MailError::InvalidArgument(format!("{field} address {s:?}: {e}")))
}

fn bracket(id: &str) -> String {
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    format!("<{id}>")
}

/// Render Markdown to an HTML fragment. Raw HTML in the input is escaped, images become links.
pub fn markdown_to_html(markdown: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let events = Parser::new_ext(markdown, options).map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }),
        Event::End(TagEnd::Image) => Event::End(TagEnd::Link),
        other => other,
    });
    let mut html = String::with_capacity(markdown.len() * 3 / 2);
    pulldown_cmark::html::push_html(&mut html, events);
    html
}

fn html_document(fragment: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"></head><body>\n{fragment}</body></html>\n"
    )
}

/// A built message plus the bare recipient addresses (for the send policy and the SMTP envelope).
#[derive(Debug, Clone)]
pub struct Built {
    /// The lettre message.
    pub message: Message,
    /// Bare To + Cc addresses.
    pub recipients: Vec<String>,
}

impl Built {
    /// RFC 5322 bytes.
    pub fn bytes(&self) -> Vec<u8> {
        self.message.formatted()
    }
}

/// Build the message. A reply with no `to` goes to the original's Reply-To or From.
pub fn build(o: &Outgoing) -> Result<Built> {
    let mut to = o.to.clone();
    if to.is_empty()
        && let Some(addr) = o.reply.as_ref().and_then(|r| r.address.clone())
    {
        to.push(addr);
    }
    if to.is_empty() {
        return Err(MailError::InvalidArgument(
            "at least one `to` recipient is needed".into(),
        ));
    }
    let subject = match (&o.subject, &o.reply) {
        (Some(s), _) if !s.trim().is_empty() => s.clone(),
        (_, Some(r)) => {
            let orig = r.subject.clone().unwrap_or_default();
            if orig.to_ascii_lowercase().starts_with("re:") {
                orig
            } else {
                format!("Re: {orig}")
            }
        }
        _ => String::new(),
    };

    let mut b = Message::builder()
        .from(mailbox("from", &o.from)?)
        .subject(subject)
        .date_now()
        .message_id(None)
        .header(XMailer(X_MAILER.to_string()));
    let mut recipients = Vec::new();
    for t in &to {
        let m = mailbox("to", t)?;
        recipients.push(m.email.to_string());
        b = b.to(m);
    }
    for c in &o.cc {
        let m = mailbox("cc", c)?;
        recipients.push(m.email.to_string());
        b = b.cc(m);
    }
    if let Some(r) = &o.reply
        && let Some(id) = &r.message_id
    {
        let mut refs: Vec<String> = r.references.iter().map(|x| bracket(x)).collect();
        refs.push(bracket(id));
        b = b.in_reply_to(bracket(id)).references(refs.join(" "));
    }
    let html = html_document(&markdown_to_html(&o.body));
    let message = b
        .multipart(MultiPart::alternative_plain_html(o.body.clone(), html))
        .map_err(|e| MailError::InvalidArgument(format!("message could not be built: {e}")))?;
    Ok(Built {
        message,
        recipients,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_renders() {
        let h = markdown_to_html("# Hi\n\n**bold** and *it*\n\n- a\n- b\n");
        assert!(h.contains("<h1>Hi</h1>"), "{h}");
        assert!(h.contains("<strong>bold</strong>"), "{h}");
        assert!(h.contains("<li>a</li>"), "{h}");
    }

    #[test]
    fn raw_html_is_escaped() {
        let h = markdown_to_html("hi <script>alert(1)</script>\n\n<div onclick=x>block</div>\n");
        assert!(!h.contains("<script>") && !h.contains("<div"), "{h}");
        assert!(h.contains("&lt;script&gt;") && h.contains("&lt;div"), "{h}");
    }

    #[test]
    fn images_become_links() {
        let h = markdown_to_html("![pixel](https://tracker.example/p.gif?d=secret)");
        assert!(!h.contains("<img"), "{h}");
        assert!(
            h.contains(r#"<a href="https://tracker.example/p.gif?d=secret">pixel</a>"#),
            "{h}"
        );
    }
}
