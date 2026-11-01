//! Outgoing MIME: headers, reply threading and the X-Mailer stamp. The built bytes are parsed
//! back with the same parser `read_message` uses.

use emperor_mail::compose::{Outgoing, ReplyTo, X_MAILER, build};
use emperor_mail::message::parse_message;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn plain_message_has_x_mailer_and_recipients() {
    let built = build(&Outgoing {
        from: "ann@example.net".into(),
        to: vec!["Bob <bob@example.org>".into()],
        cc: vec!["carol@example.org".into()],
        subject: Some("Lunch".into()),
        body: "**Friday** at noon?\n\n- pizza\n- salad".into(),
        reply: None,
    })
    .unwrap();
    let raw = built.bytes();
    let s = text(&raw);
    assert!(s.contains(&format!("X-Mailer: {X_MAILER}")), "{s}");
    assert!(X_MAILER.starts_with("emperor-mail/"));
    assert!(s.contains("Content-Type: text/plain; charset=utf-8"), "{s}");
    assert!(s.contains("Content-Type: text/html; charset=utf-8"), "{s}");
    assert!(!s.contains("In-Reply-To"), "{s}");
    assert_eq!(
        built.recipients,
        vec!["bob@example.org", "carol@example.org"]
    );

    let back = parse_message(&raw).unwrap();
    assert_eq!(back.headers.subject.as_deref(), Some("Lunch"));
    assert_eq!(back.headers.to, vec!["Bob <bob@example.org>"]);
    assert!(back.body.contains("**Friday** at noon?"));
    assert!(back.headers.message_id.is_some());
}

#[test]
fn reply_sets_threading_headers_and_defaults() {
    let built = build(&Outgoing {
        from: "ann@example.net".into(),
        to: vec![],
        cc: vec![],
        subject: None,
        body: "Thanks, received.".into(),
        reply: Some(ReplyTo {
            message_id: Some("alt-1@example.org".into()),
            references: vec!["root-0@example.org".into()],
            subject: Some("Quarterly report".into()),
            address: Some("zoe@example.org".into()),
        }),
    })
    .unwrap();
    let s = text(&built.bytes());
    assert!(s.contains("In-Reply-To: <alt-1@example.org>"), "{s}");
    assert!(
        s.contains("References: <root-0@example.org> <alt-1@example.org>"),
        "{s}"
    );
    assert!(s.contains("Subject: Re: Quarterly report"), "{s}");
    assert_eq!(built.recipients, vec!["zoe@example.org"]);
}

#[test]
fn reply_does_not_stack_re_prefixes() {
    let built = build(&Outgoing {
        from: "ann@example.net".into(),
        to: vec!["zoe@example.org".into()],
        body: "ok".into(),
        reply: Some(ReplyTo {
            message_id: Some("<x@example.org>".into()),
            subject: Some("RE: Quarterly report".into()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    let s = text(&built.bytes());
    assert!(s.contains("Subject: RE: Quarterly report"), "{s}");
    assert!(s.contains("In-Reply-To: <x@example.org>"), "{s}");
}

#[test]
fn header_injection_in_subject_stays_one_header() {
    let built = build(&Outgoing {
        from: "ann@example.net".into(),
        to: vec!["bob@example.org".into()],
        subject: Some("hi\r\nBcc: attacker@example.com".into()),
        body: "x".into(),
        ..Default::default()
    })
    .unwrap();
    let s = text(&built.bytes());
    assert!(
        !s.lines().any(|l| l.starts_with("Bcc:")),
        "a CRLF in the subject must not create a header: {s}"
    );
    assert_eq!(built.recipients, vec!["bob@example.org"]);
}

#[test]
fn bad_addresses_and_missing_recipients_are_errors() {
    let no_to = build(&Outgoing {
        from: "ann@example.net".into(),
        body: "x".into(),
        ..Default::default()
    })
    .unwrap_err();
    assert!(no_to.to_string().contains("`to`"), "{no_to}");
    let bad = build(&Outgoing {
        from: "ann@example.net".into(),
        to: vec!["not an address".into()],
        body: "x".into(),
        ..Default::default()
    })
    .unwrap_err();
    assert!(bad.to_string().contains("not an address"), "{bad}");
}

#[test]
fn markdown_body_is_multipart_alternative() {
    use mail_parser::{MessageParser, MimeHeaders, PartType};

    let body = "# Plan\n\n**Friday** at noon <script>alert(1)</script>\n\n![x](https://pixel.example/a.gif)\n";
    let built = build(&Outgoing {
        from: "ann@example.net".into(),
        to: vec!["bob@example.org".into()],
        subject: Some("Plan".into()),
        body: body.into(),
        ..Default::default()
    })
    .unwrap();
    let raw = built.bytes();
    let m = MessageParser::default().parse(&raw).unwrap();

    let root = m.root_part();
    let ct = root.content_type().unwrap();
    assert_eq!(ct.ctype(), "multipart");
    assert_eq!(ct.subtype(), Some("alternative"));
    let PartType::Multipart(children) = &root.body else {
        panic!("root is not multipart");
    };
    assert_eq!(children.len(), 2);

    let plain = &m.parts[children[0] as usize];
    let html = &m.parts[children[1] as usize];
    assert_eq!(plain.content_type().unwrap().subtype(), Some("plain"));
    assert_eq!(html.content_type().unwrap().subtype(), Some("html"));
    assert_eq!(plain.text_contents().unwrap().replace("\r\n", "\n"), body);
    let h = html.text_contents().unwrap();
    assert!(h.contains("<h1>Plan</h1>"), "{h}");
    assert!(h.contains("<strong>Friday</strong>"), "{h}");
    assert!(
        !h.contains("<script>") && h.contains("&lt;script&gt;"),
        "{h}"
    );
    assert!(!h.contains("<img"), "{h}");

    // read_message shows the text part, as written.
    let back = parse_message(&raw).unwrap();
    assert_eq!(back.body_source, "text");
    assert!(back.body.contains("**Friday** at noon"), "{}", back.body);
}
