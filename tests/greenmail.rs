//! Live IMAP + SMTP against a GreenMail container: search, read, read an attachment, draft a
//! reply, send (Markdown as multipart/alternative), the copy in the sent folder, and the send
//! guard, all through the HTTP server with the login in forwarded headers.
//!
//! Ignored by default because it needs Docker. Run it with:
//!
//! ```text
//! docker pull greenmail/standalone:2.1.5
//! cargo test --test greenmail -- --ignored --nocapture
//! ```
//!
//! The test starts its own container on random loopback ports with a user name and password made
//! up for this run, and removes the container at the end. GreenMail's TLS ports use a self-signed
//! certificate, which emperor-mail rightly refuses, so the test uses the plain ports (allowed
//! only for loopback hosts, and only from the environment: the server's environment names the
//! container, the requests carry the login).

use std::net::SocketAddr;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use emperor_mail::config::{Credentials, Field, Settings};
use emperor_mail::{MailServer, SendPolicy, forward_config, imap};
use emperor_mcp::{ServeOptions, serve};
use serde_json::{Value, json};

const IMAGE: &str = "greenmail/standalone:2.1.5";

struct Container(String);

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
    }
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().expect("docker");
    assert!(
        out.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn host_port(container: &str, port: &str) -> u16 {
    let mapped = docker(&["port", container, port]);
    mapped
        .lines()
        .next()
        .and_then(|l| l.rsplit(':').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no mapping for {port}: {mapped}"))
}

fn run_id() -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}{:x}", n, std::process::id())
}

async fn wait_for_port(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("GreenMail did not open port {port}");
}

async fn start_server(env: Settings, policy: SendPolicy) -> String {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let server = MailServer::new(env, policy);
    tokio::spawn(serve(
        ServeOptions::new(addr).with_forward(forward_config()),
        move || server.clone(),
    ));
    let base = format!("http://{addr}");
    for _ in 0..50 {
        if reqwest::get(format!("{base}/health")).await.is_ok() {
            return base;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not start");
}

async fn call(base: &str, headers: &[(String, String)], tool: &str, args: Value) -> Value {
    let mut req = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-06-18")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                      "params":{"name":tool,"arguments":args}}));
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let text = req.send().await.unwrap().text().await.unwrap();
    let body = text
        .lines()
        .find_map(|l| l.strip_prefix("data:"))
        .unwrap_or(&text)
        .trim()
        .to_string();
    let v: Value = serde_json::from_str(&body).unwrap();
    v["result"].clone()
}

fn texts(r: &Value) -> String {
    r["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn ok(r: &Value, what: &str) {
    assert_eq!(r["isError"], false, "{what}: {}", texts(r));
}

#[tokio::test]
#[ignore = "needs Docker and the greenmail/standalone image; see the file header"]
async fn greenmail_end_to_end() {
    // A mailbox that exists only for this run.
    let id = run_id();
    let user = format!("u{id}@example.test");
    let password = format!("p{id}");
    let name = format!("emperor-mail-greenmail-{id}");
    let opts = format!(
        "-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 \
         -Dgreenmail.users=u{id}:{password}@example.test -Dgreenmail.users.login=email"
    );
    docker(&[
        "run",
        "-d",
        "--name",
        &name,
        "-e",
        &format!("GREENMAIL_OPTS={opts}"),
        "-p",
        "127.0.0.1::3143",
        "-p",
        "127.0.0.1::3025",
        IMAGE,
    ]);
    let _guard = Container(name.clone());
    let imap_port = host_port(&name, "3143/tcp");
    let smtp_port = host_port(&name, "3025/tcp");
    wait_for_port(imap_port).await;
    wait_for_port(smtp_port).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let endpoints = [
        (Field::ImapHost, "127.0.0.1".to_string()),
        (Field::ImapPort, imap_port.to_string()),
        (Field::ImapSecurity, "plain".into()),
        (Field::SmtpHost, "127.0.0.1".into()),
        (Field::SmtpPort, smtp_port.to_string()),
        (Field::SmtpSecurity, "plain".into()),
    ];
    let env = Settings::from_lookup(|f| {
        endpoints
            .iter()
            .find(|(k, _)| *k == f)
            .map(|(_, v)| v.clone())
    });
    let headers: Vec<(String, String)> = [
        (Field::User, user.clone()),
        (Field::Password, password.clone()),
    ]
    .into_iter()
    .map(|(f, v)| (f.header().to_string(), v))
    .collect();
    let creds = Credentials::resolve(
        &env,
        &Settings::from_lookup(|f| {
            headers
                .iter()
                .find(|(k, _)| k == f.header())
                .map(|(_, v)| v.clone())
        }),
        false,
    )
    .unwrap();

    // Deliver the fixture into INBOX (unread).
    let fixture = std::fs::read(format!(
        "{}/tests/fixtures/with-attachments.eml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    imap::with_session(&creds, |mut s| async move {
        let r = s
            .append("INBOX", None, None, &fixture)
            .await
            .map_err(Into::into);
        (s, r)
    })
    .await
    .unwrap();

    let policy = SendPolicy::new(true, &["@example.test".into()]).unwrap();
    let base = start_server(env, policy).await;

    // list_folders
    let r = call(&base, &headers, "list_folders", json!({})).await;
    ok(&r, "list_folders");
    assert!(texts(&r).contains("INBOX"));

    // search
    let r = call(&base, &headers, "search", json!({"subject":"Invoice"})).await;
    ok(&r, "search");
    let row = &r["structuredContent"]["messages"][0];
    let uid = row["uid"].as_u64().expect("uid");
    assert_eq!(row["has_attachments"], true, "{r}");
    assert_eq!(row["unseen"], true, "{r}");
    assert_eq!(row["subject"], "Invoice 2026-117");

    // read_message leaves it unread
    let r = call(&base, &headers, "read_message", json!({"uid": uid})).await;
    ok(&r, "read_message");
    let t = texts(&r);
    assert!(t.contains("Please find the invoice attached."), "{t}");
    assert!(t.contains("invoice-2026-117.pdf"), "{t}");
    let r = call(&base, &headers, "search", json!({"unseen": true})).await;
    assert_eq!(
        r["structuredContent"]["total"], 1,
        "BODY.PEEK must not mark read: {r}"
    );

    // read_attachment: the PDF
    let r = call(
        &base,
        &headers,
        "read_attachment",
        json!({"uid": uid, "index": 0}),
    )
    .await;
    ok(&r, "read_attachment");
    assert!(
        texts(&r).contains("Invoice 2026-117 total EUR 4200"),
        "{}",
        texts(&r)
    );

    // draft a reply
    let r = call(
        &base,
        &headers,
        "draft",
        json!({"in_reply_to": uid, "body": "Paid today."}),
    )
    .await;
    ok(&r, "draft");
    let drafts = r["structuredContent"]["folder"]
        .as_str()
        .unwrap()
        .to_string();
    let r = call(&base, &headers, "search", json!({"folder": drafts})).await;
    ok(&r, "search drafts");
    let draft_uid = r["structuredContent"]["messages"][0]["uid"]
        .as_u64()
        .unwrap() as u32;
    let raw = imap::with_session(&creds, |mut s| {
        let drafts = drafts.clone();
        async move {
            let r = imap::fetch_message(&mut s, &drafts, draft_uid, false).await;
            (s, r)
        }
    })
    .await
    .unwrap();
    let raw = String::from_utf8_lossy(&raw);
    assert!(raw.contains("In-Reply-To: <inv-117@example.com>"), "{raw}");
    assert!(raw.contains("Subject: Re: Invoice 2026-117"), "{raw}");
    assert!(raw.contains("X-Mailer: emperor-mail/"), "{raw}");

    // send without a sent folder: delivered, with a warning instead of a copy
    let r = call(
        &base,
        &headers,
        "send",
        json!({"to": [user], "subject": "emperor-mail first", "body": "no sent folder yet"}),
    )
    .await;
    ok(&r, "send without sent folder");
    assert_eq!(r["structuredContent"]["sent"], true, "{r}");
    if r["structuredContent"]["sent_folder"].is_null() {
        assert!(
            r["structuredContent"]["warning"]
                .as_str()
                .is_some_and(|w| w.contains("sent folder")),
            "{r}"
        );
    }

    // with a Sent folder the exact message is copied there
    let _ = imap::with_session(&creds, |mut s| async move {
        let r = s.create("Sent").await.map_err(Into::into);
        (s, r)
    })
    .await;
    let r = call(
        &base,
        &headers,
        "send",
        json!({"to": [user], "subject": "emperor-mail live check",
               "body": "sent by **the test**"}),
    )
    .await;
    ok(&r, "send");
    let sent_folder = r["structuredContent"]["sent_folder"]
        .as_str()
        .unwrap_or_else(|| panic!("no sent copy: {r}"))
        .to_string();
    assert!(r["structuredContent"]["warning"].is_null(), "{r}");
    let message_id = r["structuredContent"]["message_id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = call(
        &base,
        &headers,
        "search",
        json!({"folder": sent_folder, "subject": "live check"}),
    )
    .await;
    ok(&r, "search sent");
    assert_eq!(r["structuredContent"]["total"], 1, "{r}");
    let copy_uid = r["structuredContent"]["messages"][0]["uid"]
        .as_u64()
        .unwrap() as u32;
    let copy = imap::with_session(&creds, |mut s| {
        let folder = sent_folder.clone();
        async move {
            let r = imap::fetch_message(&mut s, &folder, copy_uid, false).await;
            (s, r)
        }
    })
    .await
    .unwrap();
    let copy = String::from_utf8_lossy(&copy);
    assert!(
        copy.contains(&format!("Message-ID: {message_id}")),
        "{copy}"
    );
    assert!(copy.contains("multipart/alternative"), "{copy}");
    assert!(copy.contains("<strong>the test</strong>"), "{copy}");

    let mut found = Value::Null;
    for _ in 0..40 {
        let r = call(&base, &headers, "search", json!({"subject": "live check"})).await;
        if r["structuredContent"]["total"] == 1 {
            found = r;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let sent_uid = found["structuredContent"]["messages"][0]["uid"]
        .as_u64()
        .expect("sent message arrived");
    let r = call(
        &base,
        &headers,
        "read_message",
        json!({"uid": sent_uid, "mark_read": true}),
    )
    .await;
    ok(&r, "read sent");
    assert!(texts(&r).contains("sent by **the test**"));
    let r = call(&base, &headers, "search", json!({"unseen": true})).await;
    assert_eq!(
        r["structuredContent"]["total"], 2,
        "mark_read sets \\Seen (the fixture and the first send stay unread): {r}"
    );

    // send outside the allow-list is refused
    let r = call(
        &base,
        &headers,
        "send",
        json!({"to": ["someone@example.com"], "subject": "no", "body": "no"}),
    )
    .await;
    assert_eq!(r["isError"], true);
    assert!(texts(&r).contains("draft"));
}
