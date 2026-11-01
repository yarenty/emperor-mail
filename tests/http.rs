//! The server over real Streamable HTTP (`emperor_mcp::serve`), as a client sees it: the tool
//! list, the send guard, credentials arriving in forwarded headers, and the errors a caller gets
//! without them. A small scripted IMAP server on loopback stands in for the mail server so the
//! test sees exactly what the tool sent upstream.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use emperor_mail::config::{Field, Settings};
use emperor_mail::{MailServer, SendPolicy, forward_config};
use emperor_mcp::{ServeOptions, serve};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn settings(pairs: &[(Field, String)]) -> Settings {
    Settings::from_lookup(|f| pairs.iter().find(|(k, _)| *k == f).map(|(_, v)| v.clone()))
}

async fn start(env: Settings, policy: SendPolicy) -> String {
    let addr = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let server = MailServer::new(env, policy);
    let options = ServeOptions::new(addr).with_forward(forward_config());
    tokio::spawn(serve(options, move || server.clone()));
    let base = format!("http://{addr}");
    for _ in 0..50 {
        if reqwest::get(format!("{base}/health")).await.is_ok() {
            return base;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("server did not start");
}

async fn rpc(base: &str, body: Value, headers: &[(&str, &str)]) -> Value {
    let mut req = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-06-18")
        .json(&body);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.unwrap();
    assert!(resp.status().is_success(), "status {}", resp.status());
    let text = resp.text().await.unwrap();
    let json = text
        .lines()
        .find_map(|l| l.strip_prefix("data:"))
        .unwrap_or(&text)
        .trim()
        .to_string();
    serde_json::from_str(&json).unwrap_or_else(|e| panic!("bad body {text}: {e}"))
}

async fn call(base: &str, tool: &str, args: Value, headers: &[(&str, &str)]) -> Value {
    let r = rpc(
        base,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args}}),
        headers,
    )
    .await;
    r["result"].clone()
}

fn texts(result: &Value) -> String {
    result["content"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// A scripted IMAP server: greets, records every command line, accepts or refuses LOGIN, and
/// answers LIST / STATUS / LOGOUT. Returns its port and the recorded lines.
async fn scripted_imap(accept_login: bool) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut lines = BufReader::new(r).lines();
                w.write_all(b"* OK [CAPABILITY IMAP4rev1] scripted ready\r\n")
                    .await
                    .unwrap();
                while let Ok(Some(line)) = lines.next_line().await {
                    log.lock().unwrap().push(line.clone());
                    let mut parts = line.splitn(3, ' ');
                    let tag = parts.next().unwrap_or("*").to_string();
                    let cmd = parts.next().unwrap_or("").to_ascii_uppercase();
                    let rest = parts.next().unwrap_or("");
                    let reply = match cmd.as_str() {
                        "LOGIN" if accept_login => format!("{tag} OK logged in\r\n"),
                        "LOGIN" => {
                            format!("{tag} NO [AUTHENTICATIONFAILED] invalid credentials\r\n")
                        }
                        "LIST" => format!(
                            "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
                             * LIST (\\HasNoChildren \\Drafts) \"/\" \"Drafts\"\r\n\
                             * LIST (\\Noselect \\HasChildren) \"/\" \"Archive\"\r\n\
                             {tag} OK LIST done\r\n"
                        ),
                        "STATUS" => {
                            let name = rest.split('"').nth(1).unwrap_or("INBOX");
                            let (m, u) = if name == "INBOX" { (3, 1) } else { (0, 0) };
                            format!(
                                "* STATUS \"{name}\" (MESSAGES {m} UNSEEN {u})\r\n{tag} OK STATUS done\r\n"
                            )
                        }
                        "LOGOUT" => format!("* BYE\r\n{tag} OK bye\r\n"),
                        _ => format!("{tag} BAD unexpected\r\n"),
                    };
                    if w.write_all(reply.as_bytes()).await.is_err() || cmd == "LOGOUT" {
                        break;
                    }
                }
            });
        }
    });
    (port, seen)
}

#[tokio::test]
async fn tools_list_has_exactly_six_tools() {
    let base = start(Settings::default(), SendPolicy::default()).await;
    let list = rpc(
        &base,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        &[],
    )
    .await;
    let mut names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "draft",
            "list_folders",
            "read_attachment",
            "read_message",
            "search",
            "send"
        ]
    );
    assert_eq!(MailServer::tool_names().len(), 6);
}

#[tokio::test]
async fn send_is_refused_without_allow_send() {
    let base = start(Settings::default(), SendPolicy::default()).await;
    let r = call(
        &base,
        "send",
        json!({"to":["bob@example.org"],"subject":"hi","body":"hello"}),
        &[],
    )
    .await;
    assert_eq!(r["isError"], true, "{r}");
    let t = texts(&r);
    assert!(t.contains("--allow-send") && t.contains("`draft`"), "{t}");
}

#[tokio::test]
async fn send_is_refused_for_recipients_outside_the_allow_list() {
    let policy = SendPolicy::new(true, &["@example.org".into()]).unwrap();
    let base = start(Settings::default(), policy).await;
    let headers = [
        ("X-Mail-User", "ann@example.org"),
        ("X-Mail-Password", "pw-from-header"),
        ("X-Imap-Host", "imap.example.org"),
        ("X-Smtp-Host", "smtp.example.org"),
    ];
    let r = call(
        &base,
        "send",
        json!({"to":["bob@example.org","eve@elsewhere.net"],"subject":"hi","body":"x"}),
        &headers,
    )
    .await;
    assert_eq!(r["isError"], true, "{r}");
    let t = texts(&r);
    assert!(
        t.contains("eve@elsewhere.net") && t.contains("`draft`"),
        "{t}"
    );
    assert!(!t.contains("bob@example.org not"), "{t}");
}

#[tokio::test]
async fn header_credentials_reach_the_mail_server() {
    let (port, seen) = scripted_imap(true).await;
    let base = start(Settings::default(), SendPolicy::default()).await;
    let port = port.to_string();
    let headers = [
        ("X-Mail-User", "ann@example.net"),
        ("X-Mail-Password", "s3cret \"quoted\""),
        ("X-Imap-Host", "127.0.0.1"),
        ("X-Imap-Port", port.as_str()),
        ("X-Imap-Security", "plain"),
    ];
    let r = call(&base, "list_folders", json!({}), &headers).await;
    assert_eq!(r["isError"], false, "{r}");

    let lines = seen.lock().unwrap().clone();
    let login = lines.iter().find(|l| l.contains("LOGIN")).expect("LOGIN");
    assert!(
        login.ends_with(r#"LOGIN "ann@example.net" "s3cret \"quoted\"""#),
        "{login}"
    );

    let folders = &r["structuredContent"]["folders"];
    assert_eq!(folders[0]["name"], "INBOX", "{r}");
    assert_eq!(folders[0]["messages"], 3);
    assert_eq!(folders[0]["unseen"], 1);
    assert_eq!(folders[1]["special_use"], "drafts");
    assert_eq!(
        folders[2]["messages"],
        Value::Null,
        "\\Noselect has no counts"
    );
    let t = texts(&r);
    assert!(
        t.contains("BEGIN_DATA_") && t.contains("computed result"),
        "{t}"
    );
}

#[tokio::test]
async fn operator_endpoint_pins_the_host() {
    let (port, seen) = scripted_imap(true).await;
    let env = settings(&[
        (Field::ImapHost, "127.0.0.1".into()),
        (Field::ImapPort, port.to_string()),
        (Field::ImapSecurity, "plain".into()),
    ]);
    let base = start(env, SendPolicy::default()).await;
    let headers = [
        ("X-Mail-User", "bob"),
        ("X-Mail-Password", "pw"),
        // Ignored: the operator pinned the IMAP server.
        ("X-Imap-Host", "192.0.2.1"),
    ];
    let r = call(&base, "list_folders", json!({}), &headers).await;
    assert_eq!(r["isError"], false, "{r}");
    assert!(seen.lock().unwrap().iter().any(|l| l.contains("\"bob\"")));
}

#[tokio::test]
async fn refused_login_is_reported_without_the_password() {
    let (port, _) = scripted_imap(false).await;
    let base = start(Settings::default(), SendPolicy::default()).await;
    let port = port.to_string();
    let headers = [
        ("X-Mail-User", "ann@example.net"),
        ("X-Mail-Password", "do-not-echo-me"),
        ("X-Imap-Host", "localhost"),
        ("X-Imap-Port", port.as_str()),
        ("X-Imap-Security", "plain"),
    ];
    let r = call(&base, "search", json!({"subject":"x"}), &headers).await;
    assert_eq!(r["isError"], true, "{r}");
    let t = texts(&r);
    assert!(t.contains("login as ann@example.net failed"), "{t}");
    assert!(!t.contains("do-not-echo-me"), "{t}");
}

#[tokio::test]
async fn missing_credentials_are_a_clear_error() {
    let base = start(Settings::default(), SendPolicy::default()).await;
    for (tool, args) in [
        ("search", json!({})),
        ("read_message", json!({"uid": 1})),
        ("draft", json!({"to":["a@example.org"],"body":"x"})),
    ] {
        let r = call(&base, tool, args, &[]).await;
        assert_eq!(r["isError"], true, "{tool}: {r}");
        let t = texts(&r);
        assert!(
            t.contains("missing credentials") && t.contains("X-Mail-User"),
            "{tool}: {t}"
        );
    }
    // Only one of the pair: still refused, and the operator's env is never borrowed.
    let r = call(
        &base,
        "list_folders",
        json!({}),
        &[("X-Mail-User", "ann@example.net")],
    )
    .await;
    assert!(texts(&r).contains("X-Mail-Password"), "{r}");
}

#[tokio::test]
async fn plain_to_a_remote_host_is_refused() {
    let base = start(Settings::default(), SendPolicy::default()).await;
    let headers = [
        ("X-Mail-User", "ann@example.net"),
        ("X-Mail-Password", "pw"),
        ("X-Imap-Host", "imap.example.org"),
        ("X-Imap-Security", "plain"),
    ];
    let r = call(&base, "list_folders", json!({}), &headers).await;
    assert_eq!(r["isError"], true, "{r}");
    assert!(texts(&r).contains("loopback"), "{r}");
}

#[tokio::test]
async fn search_injection_is_refused_before_connecting() {
    let base = start(Settings::default(), SendPolicy::default()).await;
    let r = call(
        &base,
        "search",
        json!({"subject":"x\r\nA1 DELETE INBOX"}),
        &[],
    )
    .await;
    assert_eq!(r["isError"], true, "{r}");
    assert!(texts(&r).contains("line break") || texts(&r).contains("carriage return"));
}
