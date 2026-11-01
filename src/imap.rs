//! IMAP: one connection per tool call, opened, used and logged out.
//!
//! TLS is rustls with the platform verifier (the operating system's trust store); there is no
//! option to skip certificate checks. `plain` is accepted only for loopback hosts (see
//! [`crate::config::Security`]).

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_imap::imap_proto::BodyStructure;
use async_imap::types::{Fetch, NameAttribute};
use async_imap::{Client, Session};
use futures::TryStreamExt;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::ServerName;

use crate::config::{Credentials, Endpoint, Security};
use crate::error::{MailError, Result};
use crate::message::{self, Headers};

/// TCP connect + TLS handshake + greeting.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A whole tool call (connect, login, commands, logout).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(90);
/// Largest message `read_message` / `read_attachment` will download.
pub const MAX_MESSAGE_BYTES: u32 = 50 * 1024 * 1024;

/// Byte stream the IMAP client runs on: plain TCP or TLS over TCP.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send + fmt::Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + fmt::Debug> Io for T {}

/// A logged-in IMAP session.
pub type ImapSession = Session<Box<dyn Io>>;

/// rustls client config: ring provider, OS trust store.
pub fn tls_config() -> Result<Arc<ClientConfig>> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|b| b.with_platform_verifier())
        .map_err(|e| MailError::Config(format!("TLS setup: {e}")))?
        .with_no_client_auth();
    Ok(Arc::new(config))
}

async fn tls_wrap(tcp: TcpStream, ep: &Endpoint) -> Result<Box<dyn Io>> {
    let name = ServerName::try_from(ep.host.clone()).map_err(|e| MailError::Connect {
        target: ep.target(),
        detail: format!("bad TLS server name: {e}"),
    })?;
    let tls = TlsConnector::from(tls_config()?)
        .connect(name, tcp)
        .await
        .map_err(|e| MailError::Connect {
            target: ep.target(),
            detail: format!("TLS: {e}"),
        })?;
    Ok(Box::new(tls))
}

async fn greeting(client: &mut Client<Box<dyn Io>>, ep: &Endpoint) -> Result<()> {
    match client.read_response().await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(MailError::Connect {
            target: ep.target(),
            detail: "server closed the connection before greeting".into(),
        }),
        Err(e) => Err(MailError::Connect {
            target: ep.target(),
            detail: e.to_string(),
        }),
    }
}

async fn connect_inner(ep: &Endpoint) -> Result<Client<Box<dyn Io>>> {
    let tcp = TcpStream::connect((ep.host.as_str(), ep.port))
        .await
        .map_err(|e| MailError::Connect {
            target: ep.target(),
            detail: e.to_string(),
        })?;
    match ep.security {
        Security::Tls => {
            let mut client = Client::new(tls_wrap(tcp, ep).await?);
            greeting(&mut client, ep).await?;
            Ok(client)
        }
        Security::Plain => {
            let mut client = Client::new(Box::new(tcp) as Box<dyn Io>);
            greeting(&mut client, ep).await?;
            Ok(client)
        }
        Security::StartTls => {
            let mut client = Client::new(tcp);
            match client.read_response().await {
                Ok(Some(_)) => {}
                _ => {
                    return Err(MailError::Connect {
                        target: ep.target(),
                        detail: "no greeting".into(),
                    });
                }
            }
            client
                .run_command_and_check_ok("STARTTLS", None)
                .await
                .map_err(|e| MailError::Connect {
                    target: ep.target(),
                    detail: format!("STARTTLS refused: {e}"),
                })?;
            Ok(Client::new(tls_wrap(client.into_inner(), ep).await?))
        }
    }
}

/// Connect (bounded by [`CONNECT_TIMEOUT`]) and log in.
pub async fn open(creds: &Credentials) -> Result<ImapSession> {
    let client = tokio::time::timeout(CONNECT_TIMEOUT, connect_inner(&creds.imap))
        .await
        .map_err(|_| MailError::Timeout {
            what: "IMAP connect",
            after: CONNECT_TIMEOUT,
        })??;
    client
        .login(&creds.user, creds.password.expose())
        .await
        .map_err(|(e, _)| MailError::Login {
            user: creds.user.clone(),
            detail: e.to_string(),
        })
}

/// Run `op` on a fresh session and log out; the whole call is bounded by [`CALL_TIMEOUT`].
/// `op` takes the session by value and hands it back with its result, so the future borrows
/// nothing but what the caller captured.
pub async fn with_session<T, F, Fut>(creds: &Credentials, op: F) -> Result<T>
where
    F: FnOnce(ImapSession) -> Fut,
    Fut: Future<Output = (ImapSession, Result<T>)>,
{
    tokio::time::timeout(CALL_TIMEOUT, async {
        let session = open(creds).await?;
        let (mut session, result) = op(session).await;
        let _ = session.logout().await;
        result
    })
    .await
    .map_err(|_| MailError::Timeout {
        what: "IMAP call",
        after: CALL_TIMEOUT,
    })?
}

/// A folder as `list_folders` shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Folder {
    /// Name as the server spells it (use it verbatim as `folder`).
    pub name: String,
    /// RFC 6154 special use (`drafts`, `sent`, `trash`, `junk`, `archive`, `all`, `flagged`).
    pub special_use: Option<&'static str>,
    /// Number of messages (absent for folders that cannot be opened).
    pub messages: Option<u32>,
    /// Number of unread messages.
    pub unseen: Option<u32>,
}

fn special_use(attrs: &[NameAttribute<'_>]) -> Option<&'static str> {
    attrs.iter().find_map(|a| match a {
        NameAttribute::Drafts => Some("drafts"),
        NameAttribute::Sent => Some("sent"),
        NameAttribute::Trash => Some("trash"),
        NameAttribute::Junk => Some("junk"),
        NameAttribute::Archive => Some("archive"),
        NameAttribute::All => Some("all"),
        NameAttribute::Flagged => Some("flagged"),
        _ => None,
    })
}

struct Listed {
    name: String,
    special: Option<&'static str>,
    selectable: bool,
}

async fn list(session: &mut ImapSession) -> Result<Vec<Listed>> {
    let names: Vec<_> = session
        .list(Some(""), Some("*"))
        .await?
        .try_collect()
        .await?;
    Ok(names
        .iter()
        .map(|n| Listed {
            name: n.name().to_string(),
            special: special_use(n.attributes()),
            selectable: !n
                .attributes()
                .iter()
                .any(|a| matches!(a, NameAttribute::NoSelect)),
        })
        .collect())
}

/// Every folder with its counts.
pub async fn list_folders(session: &mut ImapSession) -> Result<Vec<Folder>> {
    let mut out = Vec::new();
    for l in list(session).await? {
        let (messages, unseen) = if l.selectable {
            match session.status(&l.name, "(MESSAGES UNSEEN)").await {
                Ok(m) => (Some(m.exists), m.unseen),
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };
        out.push(Folder {
            name: l.name,
            special_use: l.special,
            messages,
            unseen,
        });
    }
    Ok(out)
}

/// The drafts folder: the `\Drafts` special-use folder, else one called `Drafts` (at the top or
/// under INBOX), else a new `Drafts` folder is created.
pub async fn drafts_folder(session: &mut ImapSession) -> Result<String> {
    let folders = list(session).await?;
    if let Some(f) = folders.iter().find(|f| f.special == Some("drafts")) {
        return Ok(f.name.clone());
    }
    let named = |f: &&Listed| {
        let lower = f.name.to_ascii_lowercase();
        lower == "drafts" || lower.ends_with(".drafts") || lower.ends_with("/drafts")
    };
    if let Some(f) = folders.iter().find(named) {
        return Ok(f.name.clone());
    }
    session.create("Drafts").await?;
    Ok("Drafts".to_string())
}

/// Append `bytes` to `folder` flagged `\Draft \Seen`.
pub async fn append_draft(session: &mut ImapSession, folder: &str, bytes: &[u8]) -> Result<()> {
    session
        .append(folder, Some("(\\Draft \\Seen)"), None, bytes)
        .await
        .map_err(|e| MailError::Imap(format!("APPEND to {folder} failed: {e}")))
}

async fn open_folder(session: &mut ImapSession, folder: &str, writable: bool) -> Result<()> {
    let r = if writable {
        session.select(folder).await
    } else {
        session.examine(folder).await
    };
    r.map(|_| ())
        .map_err(|e| MailError::NotFound(format!("folder {folder:?} ({e})")))
}

/// One `search` result row.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Summary {
    /// IMAP UID (use with `read_message`).
    pub uid: u32,
    /// Date header (RFC 3339), else the server's arrival time.
    pub date: Option<String>,
    /// From.
    pub from: Vec<String>,
    /// Subject.
    pub subject: Option<String>,
    /// Size in bytes.
    pub size: Option<u32>,
    /// Has at least one attachment.
    pub has_attachments: bool,
    /// Not yet read.
    pub unseen: bool,
}

/// True when the structure has a part that is an attachment rather than a body.
pub fn has_attachments(bs: &BodyStructure<'_>) -> bool {
    match bs {
        BodyStructure::Multipart { bodies, .. } => bodies.iter().any(has_attachments),
        BodyStructure::Message { .. } => true,
        BodyStructure::Basic { common, .. } => !common.ty.ty.eq_ignore_ascii_case("multipart"),
        BodyStructure::Text { common, .. } => common
            .disposition
            .as_ref()
            .is_some_and(|d| d.ty.eq_ignore_ascii_case("attachment")),
    }
}

fn summary(f: &Fetch) -> Option<Summary> {
    let uid = f.uid?;
    let headers: Headers = f
        .header()
        .and_then(|h| message::parse_headers(h).ok())
        .unwrap_or_default();
    Some(Summary {
        uid,
        date: headers
            .date
            .or_else(|| f.internal_date().map(|d| d.to_rfc3339())),
        from: headers.from,
        subject: headers.subject,
        size: f.size,
        has_attachments: f.bodystructure().is_some_and(has_attachments),
        unseen: !f
            .flags()
            .any(|fl| matches!(fl, async_imap::types::Flag::Seen)),
    })
}

/// Search `folder` with an IMAP SEARCH key string; newest (highest UID) first.
/// Returns the total number of matches and up to `limit` rows.
pub async fn search(
    session: &mut ImapSession,
    folder: &str,
    query: &str,
    limit: u32,
) -> Result<(usize, Vec<Summary>)> {
    open_folder(session, folder, false).await?;
    let found: HashSet<u32> = session.uid_search(query).await?;
    let total = found.len();
    let mut uids: Vec<u32> = found.into_iter().collect();
    uids.sort_unstable_by(|a, b| b.cmp(a));
    uids.truncate(limit as usize);
    if uids.is_empty() {
        return Ok((total, Vec::new()));
    }
    let set = uids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let fetches: Vec<Fetch> = session
        .uid_fetch(
            &set,
            "(UID FLAGS RFC822.SIZE INTERNALDATE BODYSTRUCTURE BODY.PEEK[HEADER])",
        )
        .await?
        .try_collect()
        .await?;
    let mut rows: Vec<Summary> = fetches.iter().filter_map(summary).collect();
    rows.sort_unstable_by_key(|r| std::cmp::Reverse(r.uid));
    Ok((total, rows))
}

/// Download message `uid` from `folder`. With `mark_read` the folder is opened read-write and
/// `BODY[]` sets `\Seen`; otherwise it is opened read-only and `BODY.PEEK[]` leaves flags alone.
pub async fn fetch_message(
    session: &mut ImapSession,
    folder: &str,
    uid: u32,
    mark_read: bool,
) -> Result<Vec<u8>> {
    open_folder(session, folder, mark_read).await?;
    let sized: Vec<Fetch> = session
        .uid_fetch(uid.to_string(), "(UID RFC822.SIZE)")
        .await?
        .try_collect()
        .await?;
    let size = sized
        .iter()
        .find(|f| f.uid == Some(uid))
        .ok_or_else(|| MailError::NotFound(format!("message uid {uid} in {folder:?}")))?
        .size
        .unwrap_or(0);
    if size > MAX_MESSAGE_BYTES {
        return Err(MailError::InvalidArgument(format!(
            "message uid {uid} is {size} bytes; the limit is {MAX_MESSAGE_BYTES}"
        )));
    }
    let item = if mark_read {
        "(UID BODY[])"
    } else {
        "(UID BODY.PEEK[])"
    };
    let fetches: Vec<Fetch> = session
        .uid_fetch(uid.to_string(), item)
        .await?
        .try_collect()
        .await?;
    fetches
        .iter()
        .find(|f| f.uid == Some(uid))
        .and_then(|f| f.body())
        .map(<[u8]>::to_vec)
        .ok_or_else(|| MailError::NotFound(format!("message uid {uid} in {folder:?}")))
}

/// Header bytes of message `uid` (read-only, flags untouched).
pub async fn fetch_header(session: &mut ImapSession, folder: &str, uid: u32) -> Result<Vec<u8>> {
    open_folder(session, folder, false).await?;
    let fetches: Vec<Fetch> = session
        .uid_fetch(uid.to_string(), "(UID BODY.PEEK[HEADER])")
        .await?
        .try_collect()
        .await?;
    fetches
        .iter()
        .find(|f| f.uid == Some(uid))
        .and_then(|f| f.header())
        .map(<[u8]>::to_vec)
        .ok_or_else(|| MailError::NotFound(format!("message uid {uid} in {folder:?}")))
}

/// Log in, open INBOX read-only and log out: the doctor check. Returns the INBOX message count.
pub async fn check_login(creds: &Credentials) -> Result<u32> {
    with_session(creds, |mut s| async move {
        let r = s
            .examine("INBOX")
            .await
            .map(|mbox| mbox.exists)
            .map_err(|e| MailError::NotFound(format!("INBOX ({e})")));
        (s, r)
    })
    .await
}
