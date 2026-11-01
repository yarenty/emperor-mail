//! Parsing fetched messages: headers, a plain-text body, the attachment list, and the text of
//! one attachment. Pure functions over bytes; no network.

use mail_parser::{Address, MessageParser, MessagePart, MimeHeaders, PartType};
use serde::Serialize;

use crate::error::{MailError, Result};

/// Default cap on returned body or attachment text, in characters.
pub const DEFAULT_MAX_CHARS: usize = 20_000;
/// Hard cap a caller cannot exceed.
pub const MAX_CHARS: usize = 200_000;
/// HTML is rendered to text at this line width.
const HTML_WIDTH: usize = 100;
/// Largest PDF the text extractor will open.
const MAX_PDF_BYTES: usize = 25 * 1024 * 1024;
/// PDF streams are not inflated past this many bytes in total.
const MAX_PDF_DECOMPRESSED: usize = 64 * 1024 * 1024;
/// Largest office file (DOCX, XLSX, ODS, ...) the text extractors will open.
const MAX_OFFICE_BYTES: usize = 25 * 1024 * 1024;
/// `word/document.xml` is not inflated past this many bytes.
const MAX_DOCX_XML: u64 = 64 * 1024 * 1024;
/// Spreadsheet text stops after this many characters (the caller cuts further).
const MAX_SHEET_CHARS: usize = 2 * MAX_CHARS;

/// Clamp a requested character limit.
pub fn clamp_chars(max: Option<usize>) -> usize {
    max.unwrap_or(DEFAULT_MAX_CHARS).clamp(1, MAX_CHARS)
}

/// One attachment as listed by `read_message`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AttachmentInfo {
    /// Position for `read_attachment`.
    pub index: usize,
    /// File name, when the sender gave one.
    pub name: Option<String>,
    /// MIME type, e.g. `application/pdf`.
    pub content_type: String,
    /// Decoded size in bytes.
    pub size: usize,
}

/// Header fields shown for a message.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Headers {
    /// Date header as RFC 3339, when parseable.
    pub date: Option<String>,
    /// From, as `Name <address>`.
    pub from: Vec<String>,
    /// To.
    pub to: Vec<String>,
    /// Cc.
    pub cc: Vec<String>,
    /// Reply-To.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reply_to: Vec<String>,
    /// Subject.
    pub subject: Option<String>,
    /// Message-ID without angle brackets.
    pub message_id: Option<String>,
}

/// A parsed message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    /// Header fields.
    pub headers: Headers,
    /// Plain-text body (HTML-only messages are converted).
    pub body: String,
    /// `text`, `html` (converted) or `none`.
    pub body_source: &'static str,
    /// Attachments in `read_attachment` index order.
    pub attachments: Vec<AttachmentInfo>,
    /// Message-IDs from References, for threading a reply.
    pub references: Vec<String>,
    /// Bare reply address (Reply-To, else From).
    pub reply_address: Option<String>,
}

/// Text taken from one attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentText {
    /// What the attachment is.
    pub info: AttachmentInfo,
    /// Its text, or `None` for binary types.
    pub text: Option<String>,
    /// Why there is no text, or that it was cut.
    pub note: Option<String>,
}

fn parse(raw: &[u8]) -> Result<mail_parser::Message<'_>> {
    MessageParser::default()
        .parse(raw)
        .ok_or_else(|| MailError::Decode("not an RFC 5322 message".into()))
}

fn addresses(a: Option<&Address<'_>>) -> Vec<String> {
    a.map(|a| {
        a.iter()
            .map(|addr| match (addr.name(), addr.address()) {
                (Some(n), Some(e)) => format!("{n} <{e}>"),
                (None, Some(e)) => e.to_string(),
                (Some(n), None) => n.to_string(),
                (None, None) => String::new(),
            })
            .filter(|s| !s.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

fn header_fields(m: &mail_parser::Message<'_>) -> Headers {
    Headers {
        date: m.date().map(|d| d.to_rfc3339()),
        from: addresses(m.from()),
        to: addresses(m.to()),
        cc: addresses(m.cc()),
        reply_to: addresses(m.reply_to()),
        subject: m.subject().map(str::to_string),
        message_id: m.message_id().map(str::to_string),
    }
}

/// Parse only the header fields (used for `search` rows, fetched as `BODY.PEEK[HEADER]`).
pub fn parse_headers(raw: &[u8]) -> Result<Headers> {
    parse(raw).map(|m| header_fields(&m))
}

fn content_type(part: &MessagePart<'_>) -> String {
    match part.content_type() {
        Some(ct) => match ct.subtype() {
            Some(sub) => format!("{}/{}", ct.ctype(), sub).to_ascii_lowercase(),
            None => ct.ctype().to_ascii_lowercase(),
        },
        None => match part.body {
            PartType::Text(_) => "text/plain".into(),
            PartType::Html(_) => "text/html".into(),
            PartType::Message(_) => "message/rfc822".into(),
            _ => "application/octet-stream".into(),
        },
    }
}

fn info(index: usize, part: &MessagePart<'_>) -> AttachmentInfo {
    AttachmentInfo {
        index,
        name: part.attachment_name().map(str::to_string),
        content_type: content_type(part),
        size: part.len(),
    }
}

/// Render HTML as readable plain text (links kept as footnotes, tags and scripts dropped).
pub fn html_to_text(html: &str) -> String {
    html2text::config::plain_no_decorate()
        .link_footnotes(true)
        .string_from_read(html.as_bytes(), HTML_WIDTH)
        .unwrap_or_else(|_| {
            // html2text only fails on pathological input; fall back to dropping tags.
            let mut out = String::with_capacity(html.len());
            let mut in_tag = false;
            for c in html.chars() {
                match c {
                    '<' => in_tag = true,
                    '>' => in_tag = false,
                    c if !in_tag => out.push(c),
                    _ => {}
                }
            }
            out
        })
}

/// Text of every page of a PDF. CPU-bound: call from `spawn_blocking`.
pub fn pdf_to_text(bytes: &[u8]) -> Result<String> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(MailError::Decode(format!(
            "PDF is {} bytes; text is extracted from PDFs up to {MAX_PDF_BYTES} bytes",
            bytes.len()
        )));
    }
    let doc = lopdf::Document::load_mem(bytes)
        .map_err(|e| MailError::Decode(format!("PDF could not be opened: {e}")))?;
    if doc.is_encrypted() {
        return Err(MailError::Decode("PDF is encrypted".into()));
    }
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    doc.extract_text_with_limit(&pages, MAX_PDF_DECOMPRESSED)
        .map_err(|e| MailError::Decode(format!("PDF text could not be extracted: {e}")))
}

fn office_size_check(kind: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_OFFICE_BYTES {
        return Err(MailError::Decode(format!(
            "{kind} is {} bytes; text is extracted from files up to {MAX_OFFICE_BYTES} bytes",
            bytes.len()
        )));
    }
    Ok(())
}

/// Text of a Word document (DOCX): the paragraphs of `word/document.xml`, one per line, tabs
/// and line breaks kept. CPU-bound: call from `spawn_blocking`.
pub fn docx_to_text(bytes: &[u8]) -> Result<String> {
    use quick_xml::events::Event;
    use std::io::Read;

    office_size_check("DOCX", bytes)?;
    let bad = |e: &dyn std::fmt::Display| MailError::Decode(format!("DOCX could not be read: {e}"));
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| bad(&e))?;
    let entry = archive.by_name("word/document.xml").map_err(|e| bad(&e))?;
    let mut xml = String::new();
    entry
        .take(MAX_DOCX_XML)
        .read_to_string(&mut xml)
        .map_err(|e| bad(&e))?;

    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event().map_err(|e| bad(&e))? {
            Event::Start(e) if e.local_name().as_ref() == "t" => in_text = true,
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "p" => out.push('\n'),
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "tab" => out.push('\t'),
                "br" | "cr" => out.push('\n'),
                "p" => out.push('\n'),
                _ => {}
            },
            Event::Text(t) if in_text => out.push_str(&t.xml10_content()),
            Event::CData(t) if in_text => out.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if in_text => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    out.push(c);
                } else {
                    out.push_str(match r.xml10_content().as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out.trim().to_string())
}

/// Text of a spreadsheet (XLSX, XLSM, XLSB, XLS, ODS): each sheet under a `## <name>` line, one
/// row per line, cells separated by tabs. CPU-bound: call from `spawn_blocking`.
pub fn spreadsheet_to_text(bytes: &[u8]) -> Result<String> {
    use calamine::Reader;

    office_size_check("spreadsheet", bytes)?;
    let mut book = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes))
        .map_err(|e| MailError::Decode(format!("spreadsheet could not be opened: {e}")))?;
    let mut out = String::new();
    for name in book.sheet_names() {
        if out.chars().count() > MAX_SHEET_CHARS {
            break;
        }
        let Ok(range) = book.worksheet_range(&name) else {
            continue;
        };
        out.push_str(&format!("## {name}\n"));
        for row in range.rows() {
            let line = row
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join("\t");
            out.push_str(line.trim_end());
            out.push('\n');
            if out.len() > MAX_SHEET_CHARS * 4 {
                break;
            }
        }
        out.push('\n');
    }
    Ok(out.trim_end().to_string())
}

/// Normalise line endings to `\n`.
fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Cut `text` to `max` characters; returns whether it was cut.
pub fn truncate(text: &mut String, max: usize) -> bool {
    match text.char_indices().nth(max) {
        Some((byte, _)) => {
            text.truncate(byte);
            true
        }
        None => false,
    }
}

/// Document types whose text the server extracts on the blocking pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocKind {
    Pdf,
    Docx,
    Sheet,
}

fn doc_kind(info: &AttachmentInfo) -> Option<DocKind> {
    let ext = info
        .name
        .as_deref()
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase());
    match (info.content_type.as_str(), ext.as_deref()) {
        ("application/pdf", _) | (_, Some("pdf")) => Some(DocKind::Pdf),
        ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", _)
        | (_, Some("docx")) => Some(DocKind::Docx),
        ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", _)
        | ("application/vnd.ms-excel", _)
        | ("application/vnd.ms-excel.sheet.macroenabled.12", _)
        | ("application/vnd.ms-excel.sheet.binary.macroenabled.12", _)
        | ("application/vnd.oasis.opendocument.spreadsheet", _)
        | (_, Some("xlsx" | "xlsm" | "xlsb" | "xls" | "ods")) => Some(DocKind::Sheet),
        _ => None,
    }
}

fn nested_message_text(m: &mail_parser::Message<'_>) -> String {
    let h = header_fields(m);
    let (body, _) = body_text(m);
    format!(
        "From: {}\nSubject: {}\nDate: {}\n\n{body}",
        h.from.join(", "),
        h.subject.unwrap_or_default(),
        h.date.unwrap_or_default()
    )
}

fn body_text(m: &mail_parser::Message<'_>) -> (String, &'static str) {
    let mut parts = Vec::new();
    let mut source = "none";
    for part in m.text_bodies() {
        match &part.body {
            PartType::Text(t) => {
                parts.push(lf(t));
                source = "text";
            }
            PartType::Html(h) => {
                parts.push(html_to_text(h));
                if source == "none" {
                    source = "html";
                }
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        // Some senders put the only body in an HTML part that is not listed as a text body.
        if let Some(PartType::Html(h)) = m.html_bodies().next().map(|p| &p.body) {
            parts.push(html_to_text(h));
            source = "html";
        }
    }
    (parts.join("\n\n").trim().to_string(), source)
}

/// Parse a whole message (`BODY[]`).
pub fn parse_message(raw: &[u8]) -> Result<ParsedMessage> {
    let m = parse(raw)?;
    let headers = header_fields(&m);
    let (body, body_source) = body_text(&m);
    let attachments = m
        .attachments()
        .enumerate()
        .map(|(i, p)| info(i, p))
        .collect();
    let references = m
        .references()
        .as_text_list()
        .map(|l| l.iter().map(|s| s.to_string()).collect())
        .or_else(|| m.references().as_text().map(|s| vec![s.to_string()]))
        .unwrap_or_default();
    let reply_address = m
        .reply_to()
        .or_else(|| m.from())
        .and_then(|a| a.first())
        .and_then(|a| a.address())
        .map(str::to_string);
    Ok(ParsedMessage {
        headers,
        body,
        body_source,
        attachments,
        references,
        reply_address,
    })
}

/// Text of attachment `index` of a whole message, cut to `max_chars`. PDF, DOCX and spreadsheet
/// extraction runs on the blocking pool; a panic inside a parser becomes an error, not a crash.
pub async fn attachment_text(
    raw: Vec<u8>,
    index: usize,
    max_chars: usize,
) -> Result<AttachmentText> {
    enum Work {
        Done(Option<String>, Option<String>),
        Doc(DocKind, Vec<u8>),
    }
    let (info, work) = {
        let m = parse(&raw)?;
        let count = m.attachment_count();
        let part = m.attachments().nth(index).ok_or_else(|| {
            MailError::NotFound(format!(
                "attachment {index}; the message has {count} attachment(s), numbered from 0"
            ))
        })?;
        let info = info(index, part);
        let work = match &part.body {
            PartType::Text(t) => Work::Done(Some(lf(t)), None),
            PartType::Html(h) => Work::Done(Some(html_to_text(h)), None),
            PartType::Message(inner) => Work::Done(Some(nested_message_text(inner)), None),
            PartType::Binary(b) | PartType::InlineBinary(b) if doc_kind(&info).is_some() => {
                Work::Doc(doc_kind(&info).unwrap_or(DocKind::Pdf), b.to_vec())
            }
            PartType::Binary(_) | PartType::InlineBinary(_) | PartType::Multipart(_) => Work::Done(
                None,
                Some(format!(
                    "{} is binary; text is extracted from text/*, HTML, PDF, DOCX and spreadsheet attachments only",
                    info.content_type
                )),
            ),
        };
        (info, work)
    };
    let (text, note) = match work {
        Work::Done(t, n) => (t, n),
        Work::Doc(kind, bytes) => {
            let extract = move || match kind {
                DocKind::Pdf => pdf_to_text(&bytes),
                DocKind::Docx => docx_to_text(&bytes),
                DocKind::Sheet => spreadsheet_to_text(&bytes),
            };
            match tokio::task::spawn_blocking(extract).await {
                Ok(Ok(t)) => (Some(t), None),
                Ok(Err(e)) => (None, Some(e.to_string())),
                Err(_) => (
                    None,
                    Some("the document parser failed on this file".to_string()),
                ),
            }
        }
    };
    let (text, note) = match text {
        Some(mut t) => {
            if truncate(&mut t, max_chars) {
                (
                    Some(t),
                    Some(format!(
                        "text cut at {max_chars} characters; raise max_chars for more"
                    )),
                )
            } else {
                (Some(t), note)
            }
        }
        None => (None, note),
    };
    Ok(AttachmentText { info, text, note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_counts_characters_not_bytes() {
        let mut s = "zażółć".to_string();
        assert!(truncate(&mut s, 3));
        assert_eq!(s, "zaż");
        let mut short = "ab".to_string();
        assert!(!truncate(&mut short, 3));
    }

    #[test]
    fn html_drops_tags_and_scripts() {
        let t = html_to_text("<p>Hello <b>world</b></p><script>alert(1)</script>");
        assert!(t.contains("Hello") && t.contains("world"), "{t}");
        assert!(!t.contains("<b>") && !t.contains("alert"), "{t}");
    }

    #[test]
    fn garbage_pdf_is_an_error_not_a_panic() {
        assert!(pdf_to_text(b"%PDF-1.4 garbage").is_err());
    }
}
