//! The server over real Streamable HTTP (`emperor_mcp::serve`), as a client sees it: the tool
//! list in each mode, the send guard, credentials arriving in forwarded headers, and the errors a caller gets
//! without them. A small scripted IMAP server on loopback stands in for the mail server so the
//! test sees exactly what the tool sent upstream.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use emperor_mail::config::{Field, Settings};
use emperor_mail::{FromPolicy, MailServer, Mode, Options, SendPolicy, forward_config};
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
    start_with(env, policy, Options::default()).await
}

/// `--writable`: draft listed, send not.
fn writable() -> Options {
    Options {
        writable: true,
        ..Options::default()
    }
}

/// `--allow-send --send-allow @example.org`: draft and send listed.
fn sending() -> SendPolicy {
    SendPolicy::new(true, &["@example.org".into()]).unwrap()
}

async fn start_with(env: Settings, policy: SendPolicy, options: Options) -> String {
    let addr = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let server = MailServer::with_options(env, policy, options);
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

/// POST one JSON-RPC message; returns the HTTP status and the JSON body.
async fn rpc_raw(base: &str, body: Value, headers: &[(&str, &str)]) -> (u16, Value) {
    let mut req = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-06-18")
        .json(&body);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    let json = text
        .lines()
        .find_map(|l| l.strip_prefix("data:"))
        .unwrap_or(&text)
        .trim()
        .to_string();
    let value = serde_json::from_str(&json).unwrap_or_else(|e| panic!("bad body {text}: {e}"));
    (status, value)
}

async fn rpc(base: &str, body: Value, headers: &[(&str, &str)]) -> Value {
    let (status, value) = rpc_raw(base, body, headers).await;
    assert!((200..300).contains(&status), "status {status}: {value}");
    value
}

/// Environment that points IMAP at a loopback test server (plain is allowed only from the
/// environment, and only for loopback).
fn loopback_imap(port: u16) -> Settings {
    settings(&[
        (Field::ImapHost, "127.0.0.1".into()),
        (Field::ImapPort, port.to_string()),
        (Field::ImapSecurity, "plain".into()),
    ])
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
                    if line.to_ascii_uppercase().contains(" AUTHENTICATE ") {
                        let tag = line.split(' ').next().unwrap_or("*").to_string();
                        w.write_all(b"+ \r\n").await.unwrap();
                        let Ok(Some(resp)) = lines.next_line().await else {
                            break;
                        };
                        log.lock().unwrap().push(format!("SASL {resp}"));
                        let reply = if accept_login {
                            format!("{tag} OK authenticated\r\n")
                        } else {
                            format!("{tag} NO [AUTHENTICATIONFAILED] invalid token\r\n")
                        };
                        w.write_all(reply.as_bytes()).await.unwrap();
                        continue;
                    }
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

async fn list_tools(base: &str) -> Value {
    rpc(
        base,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        &[],
    )
    .await
}

fn names(list: &Value) -> Vec<String> {
    let mut names: Vec<String> = list["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    names.sort_unstable();
    names
}

const READ: [&str; 4] = ["list_folders", "read_attachment", "read_message", "search"];

#[tokio::test]
async fn default_lists_the_four_reading_tools() {
    let list = list_tools(&start(Settings::default(), SendPolicy::default()).await).await;
    assert_eq!(names(&list), READ);
    emperor_mcp::testkit::assert_read_only_default_json(&list);
    emperor_mcp::testkit::assert_read_only_default(&MailServer::tools(Mode::ReadOnly));
    assert_eq!(MailServer::tool_names(Mode::ReadOnly).len(), 4);
}

#[tokio::test]
async fn writable_adds_draft() {
    let base = start_with(Settings::default(), SendPolicy::default(), writable()).await;
    let list = list_tools(&base).await;
    let mut want = READ.to_vec();
    want.insert(0, "draft");
    assert_eq!(names(&list), want);
    assert_eq!(MailServer::tool_names(Mode::Drafts).len(), 5);
}

#[tokio::test]
async fn allow_send_adds_draft_and_send() {
    // --allow-send alone: draft comes with it.
    let list = list_tools(&start(Settings::default(), sending()).await).await;
    let mut want = READ.to_vec();
    want.insert(0, "draft");
    want.push("send");
    assert_eq!(names(&list), want);
    assert_eq!(MailServer::tool_names(Mode::DraftsAndSend).len(), 6);
}

#[tokio::test]
async fn tools_carry_annotations_and_compact_schemas() {
    for mode in [Mode::ReadOnly, Mode::Drafts, Mode::DraftsAndSend] {
        let tools = MailServer::tools(mode);
        emperor_mcp::testkit::assert_tools(&tools);
        emperor_mcp::testkit::assert_compact_schemas(&tools);
        assert!(
            emperor_mcp::testkit::warnings(&tools).is_empty(),
            "{mode:?}: {:?}",
            emperor_mcp::testkit::warnings(&tools)
        );
    }

    let list = list_tools(&start(Settings::default(), sending()).await).await;
    emperor_mcp::testkit::assert_tools_json(&list);
    emperor_mcp::testkit::assert_compact_schemas_json(&list);

    let tools = list["result"]["tools"].as_array().unwrap();
    let hints = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .map(|t| t["annotations"].clone())
            .unwrap_or_else(|| panic!("no tool {name}"))
    };
    for name in READ {
        let a = hints(name);
        assert_eq!(a["readOnlyHint"], true, "{name}: {a}");
        assert_eq!(a["openWorldHint"], true, "{name}: {a}");
    }
    let draft = hints("draft");
    assert_eq!(draft["readOnlyHint"], false, "{draft}");
    assert_eq!(draft["destructiveHint"], false, "{draft}");
    let send = hints("send");
    assert_eq!(send["readOnlyHint"], false, "{send}");
    assert_eq!(send["destructiveHint"], true, "{send}");
    assert_eq!(send["openWorldHint"], true, "{send}");
}

/// A tool that is not listed cannot be called: the answer is an error, and nothing is written.
async fn refused(base: &str, tool: &str) -> String {
    let (_, body) = rpc_raw(
        base,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,
               "arguments":{"to":["bob@example.org"],"subject":"hi","body":"hello"}}}),
        &[],
    )
    .await;
    assert!(
        body.get("error").is_some() || body["result"]["isError"] == true,
        "{tool}: {body}"
    );
    assert_ne!(
        body["result"]["structuredContent"]["drafted"], true,
        "{body}"
    );
    assert_ne!(body["result"]["structuredContent"]["sent"], true, "{body}");
    body.to_string()
}

#[tokio::test]
async fn write_tools_are_not_callable_without_their_switch() {
    let base = start(Settings::default(), SendPolicy::default()).await;
    refused(&base, "draft").await;
    refused(&base, "send").await;
    let base = start_with(Settings::default(), SendPolicy::default(), writable()).await;
    refused(&base, "send").await;
}

#[tokio::test]
async fn send_is_refused_for_recipients_outside_the_allow_list() {
    let base = start(Settings::default(), sending()).await;
    let headers = [
        ("X-Mail-User", "ann@example.org"),
        ("X-Mail-Password", "pw-from-header"),
        ("X-Mail-Imap-Host", "imap.example.org"),
        ("X-Mail-Smtp-Host", "smtp.example.org"),
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
    let base = start(loopback_imap(port), SendPolicy::default()).await;
    let headers = [
        ("X-Mail-User", "ann@example.net"),
        ("X-Mail-Password", "s3cret \"quoted\""),
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
async fn oauth_token_logs_in_with_xoauth2() {
    use base64::Engine;
    let (port, seen) = scripted_imap(true).await;
    let base = start(loopback_imap(port), SendPolicy::default()).await;
    let headers = [
        ("X-Mail-User", "ann@example.net"),
        ("X-Mail-Oauth-Token", "fake-access-token"),
    ];
    let r = call(&base, "list_folders", json!({}), &headers).await;
    assert_eq!(r["isError"], false, "{r}");
    let lines = seen.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.ends_with("AUTHENTICATE XOAUTH2")),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|l| l.contains("LOGIN")), "{lines:?}");
    let sasl = lines
        .iter()
        .find_map(|l| l.strip_prefix("SASL "))
        .expect("SASL response");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(sasl.trim())
        .unwrap();
    assert_eq!(
        decoded,
        b"user=ann@example.net\x01auth=Bearer fake-access-token\x01\x01"
    );
}

#[tokio::test]
async fn pinned_servers_ignore_header_hosts() {
    let (port, seen) = scripted_imap(true).await;
    let options = Options {
        pin_servers: true,
        ..Options::default()
    };
    let base = start_with(loopback_imap(port), SendPolicy::default(), options).await;
    let headers = [
        ("X-Mail-User", "bob"),
        ("X-Mail-Password", "pw"),
        // Ignored: the operator pinned the servers.
        ("X-Mail-Imap-Host", "192.0.2.1"),
    ];
    let r = call(&base, "list_folders", json!({}), &headers).await;
    assert_eq!(r["isError"], false, "{r}");
    assert!(seen.lock().unwrap().iter().any(|l| l.contains("\"bob\"")));
}

#[tokio::test]
async fn refused_login_is_a_401_without_the_password() {
    let (port, _) = scripted_imap(false).await;
    let base = start(loopback_imap(port), SendPolicy::default()).await;
    let headers = [
        ("X-Mail-User", "ann@example.net"),
        ("X-Mail-Password", "do-not-echo-me"),
    ];
    let (status, body) = rpc_raw(
        &base,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
               "params":{"name":"search","arguments":{"subject":"x"}}}),
        &headers,
    )
    .await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], -32040, "{body}");
    let text = body.to_string();
    assert!(text.contains("IMAP server"), "{text}");
    assert!(!text.contains("do-not-echo-me"), "{text}");

    // A refused OAuth token is the same 401.
    let headers = [
        ("X-Mail-User", "ann@example.net"),
        ("X-Mail-Oauth-Token", "expired-fake-token"),
    ];
    let (status, body) = rpc_raw(
        &base,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
               "params":{"name":"list_folders","arguments":{}}}),
        &headers,
    )
    .await;
    assert_eq!(status, 401, "{body}");
    assert!(!body.to_string().contains("expired-fake-token"), "{body}");
}

#[tokio::test]
async fn from_outside_the_allow_list_is_refused() {
    let options = Options {
        from_policy: FromPolicy::new(&["@example.net".into()]).unwrap(),
        ..Options::default()
    };
    let policy = SendPolicy::new(true, &["@example.org".into()]).unwrap();
    let base = start_with(Settings::default(), policy, options).await;
    let headers = [
        ("X-Mail-User", "ann@example.net"),
        ("X-Mail-Password", "pw"),
        ("X-Mail-From", "ceo@example.com"),
        ("X-Mail-Imap-Host", "imap.example.net"),
        ("X-Mail-Smtp-Host", "smtp.example.net"),
    ];
    for tool in ["draft", "send"] {
        let r = call(
            &base,
            tool,
            json!({"to":["bob@example.org"],"subject":"hi","body":"x"}),
            &headers,
        )
        .await;
        assert_eq!(r["isError"], true, "{tool}: {r}");
        let t = texts(&r);
        assert!(
            t.contains("ceo@example.com") && t.contains("@example.net"),
            "{tool}: {t}"
        );
    }
}

#[tokio::test]
async fn missing_credentials_are_a_clear_error() {
    let base = start_with(Settings::default(), SendPolicy::default(), writable()).await;
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
async fn plain_from_headers_is_refused() {
    let base = start(Settings::default(), SendPolicy::default()).await;
    let headers = [
        ("X-Mail-User", "ann@example.net"),
        ("X-Mail-Password", "pw"),
        ("X-Mail-Imap-Host", "imap.example.org"),
        ("X-Mail-Imap-Security", "plain"),
    ];
    let r = call(&base, "list_folders", json!({}), &headers).await;
    assert_eq!(r["isError"], true, "{r}");
    assert!(texts(&r).contains("tls or starttls only"), "{r}");
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
