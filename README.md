# emperor-mail

[![CI](https://github.com/yarenty/emperor-mail/actions/workflows/ci.yml/badge.svg)](https://github.com/yarenty/emperor-mail/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**Your mailbox as six MCP tools: search, read, draft, and send only when you allow it.**

emperor-mail is an [MCP](https://modelcontextprotocol.io) server that reads and writes email
over IMAP and SMTP, so an agent can find a message, read it and its attachments (PDF, Word,
Excel and OpenDocument spreadsheets included), and draft a reply for you to send.

- Built on [emperor-mcp](https://github.com/yarenty/emperor-mcp): stateless Streamable HTTP,
  framed tool output, forwarded credentials (Emperor Profile P1).
- Two ways to run: **one mailbox** (credentials in the environment, stdio or HTTP) or a
  **shared server** (each request brings its own credentials in headers; nothing is stored).
- **Draft by default.** `send` is off unless you start the server with `--allow-send`, and even
  then only to the recipients you list. Sent mail is copied to your Sent folder.
- Markdown bodies go out as `text/plain` plus rendered `text/html`.
- Password, app password or **OAuth 2.0** (XOAUTH2, for Gmail and Microsoft 365).
- Single binary, pure Rust: IMAP via [async-imap](https://github.com/async-email/async-imap),
  parsing via [mail-parser](https://github.com/stalwartlabs/mail-parser), SMTP via
  [lettre](https://github.com/lettre/lettre), PDF text via [lopdf](https://github.com/J-F-Liu/lopdf),
  spreadsheets via [calamine](https://github.com/tafia/calamine), Markdown via
  [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark), TLS via rustls and the
  operating system's trust store.

## Install

Prebuilt binaries (macOS arm64/x86_64, Linux x86_64, Windows x86_64) are attached to
[GitHub releases](https://github.com/yarenty/emperor-mail/releases). From source:

```bash
cargo install emperor-mail
```

Or as a container (stdio, one mailbox from the environment):

```bash
docker build -t emperor-mail https://github.com/yarenty/emperor-mail.git
docker run -i --rm -e MAIL_USER -e MAIL_PASSWORD -e IMAP_HOST -e SMTP_HOST emperor-mail
```

## Run for one mailbox

Credentials come from the environment. Use an **app password**, never your account password
(see [Provider notes](#provider-notes)).

```bash
export MAIL_USER="you@example.com"
export MAIL_PASSWORD="<app password>"
export IMAP_HOST="imap.example.com"     # port 993, implicit TLS by default
export SMTP_HOST="smtp.example.com"     # only needed for send; port 465 by default

emperor-mail doctor                     # check everything before you connect a client
emperor-mail serve --stdio              # for desktop clients that spawn the server
emperor-mail serve                      # or Streamable HTTP on 127.0.0.1:8033/mcp
```

| Variable | Meaning | Default |
|----------|---------|---------|
| `MAIL_USER` | Login name | — |
| `MAIL_PASSWORD` | Password or app password (never printed, never logged) | — |
| `MAIL_OAUTH_TOKEN` | OAuth 2.0 access token instead of a password (see [OAuth](#oauth-gmail-microsoft-365)) | — |
| `MAIL_FROM` | `From` address when the login name is not an address | the login name |
| `IMAP_HOST` / `IMAP_PORT` | IMAP server | port 993 (143 with `starttls`) |
| `IMAP_SECURITY` | `tls`, `starttls` or `plain` | `tls`, or `starttls` on port 143 |
| `SMTP_HOST` / `SMTP_PORT` | SMTP server | port 465 (587 with `starttls`) |
| `SMTP_SECURITY` | `tls`, `starttls` or `plain` | `tls`, or `starttls` on ports 587 and 25 |

`plain` (no encryption) is accepted only from the environment and only for a loopback host,
such as a local mail bridge. Certificates are always verified against the operating system's
trust store.

### Flags

`serve` and `doctor` take the same flags.

| Flag | Meaning |
|------|---------|
| `--stdio` | Speak MCP over stdin/stdout (one mailbox from the environment) |
| `--bind <addr>` | HTTP listen address (default `127.0.0.1:8033`) |
| `--allow-send` | Turn `send` on (otherwise it refuses and points to `draft`) |
| `--send-allow <pattern>` | Recipients `send` may write to: `ann@example.org` or `@example.org`; repeat or comma-separate. With `--allow-send` and no pattern, every recipient is refused |
| `--from-allow <pattern>` | From addresses `send` and `draft` may use (same syntax). Unset: any |
| `--no-sent-copy` | Do not copy sent mail to the Sent folder (for providers that file it themselves) |
| `--pin-servers` | Always use the environment's IMAP/SMTP servers; ignore server headers |

## Run as a shared server

On a shared server every request carries the caller's own credentials in headers. They are
used for that one tool call and dropped with the request: never stored, never logged.

```bash
emperor-mail serve --bind 127.0.0.1:8033
```

| Header | Same as | Notes |
|--------|---------|-------|
| `X-Mail-User` | `MAIL_USER` | Login name |
| `X-Mail-Password` | `MAIL_PASSWORD` | Secret; with `X-Mail-User` |
| `X-Mail-Oauth-Token` | `MAIL_OAUTH_TOKEN` | Secret; instead of the password (XOAUTH2) |
| `X-Mail-From` | `MAIL_FROM` | Checked against `--from-allow` |
| `X-Mail-Imap-Host`, `X-Mail-Imap-Port`, `X-Mail-Imap-Security` | `IMAP_*` | `tls` or `starttls` only |
| `X-Mail-Smtp-Host`, `X-Mail-Smtp-Port`, `X-Mail-Smtp-Security` | `SMTP_*` | `tls` or `starttls` only |

These ten names are the only headers the server captures.

Rules:

- **The login is one source.** If a request carries any of `X-Mail-User`, `X-Mail-Password` or
  `X-Mail-Oauth-Token`, the whole login comes from headers: the user plus exactly one of
  password and token. A header user is never combined with a secret from the environment.
- **Servers follow the login.** A caller with its own login may name its own servers in
  `X-Mail-Imap-*` / `X-Mail-Smtp-*`; what it leaves out falls back to the environment
  (`IMAP_HOST`, `SMTP_HOST`, ...). With a login from the environment, server headers are
  ignored, so the operator's password never goes to a host a caller picked.
- **No plaintext from headers.** A server named in a header is reached over TLS or STARTTLS
  only; `plain` is refused.
- **`--pin-servers`** makes the environment's servers win for every caller (a company server
  that should only ever reach `imap.corp.example`).
- When the mail server rejects the credentials, the HTTP response is **`401`** with a JSON-RPC
  error (code `-32040`), so a client can ask the user to sign in again.
- Put the server behind an ingress that terminates TLS and authenticates callers; the headers
  carry passwords. Without one, keep it on `127.0.0.1`.

## Provider notes

- **Gmail**: turn on 2-Step Verification, then create an app password at
  <https://myaccount.google.com/apppasswords>. `IMAP_HOST=imap.gmail.com`,
  `SMTP_HOST=smtp.gmail.com`. IMAP must be enabled in Gmail settings for some accounts. Gmail
  files sent mail in "[Gmail]/Sent Mail" by itself: run with `--no-sent-copy` to avoid a
  second copy.
- **Fastmail**: create an app password under Settings → Privacy & Security → Integrations →
  App passwords, with IMAP and SMTP access. `IMAP_HOST=imap.fastmail.com`,
  `SMTP_HOST=smtp.fastmail.com`.
- **Outlook / Microsoft 365** accounts require OAuth: see below. Exchange Online files sent mail
  by itself; use `--no-sent-copy`.

### OAuth (Gmail, Microsoft 365)

emperor-mail speaks SASL `XOAUTH2` on IMAP and SMTP. It does not run an OAuth flow and holds
no client secret: **the client obtains and refreshes the access token** and passes it per
request in `X-Mail-Oauth-Token` (or `MAIL_OAUTH_TOKEN` for stdio), with the mailbox address in
`X-Mail-User` / `MAIL_USER`. The token is forwarded for that one call and never stored or
logged. An expired token gives a `401`, the cue for the client to refresh it.

- **Gmail**: scope `https://mail.google.com/`; `IMAP_HOST=imap.gmail.com`,
  `SMTP_HOST=smtp.gmail.com`.
- **Microsoft 365**: scopes `https://outlook.office.com/IMAP.AccessAsUser.All` and
  `https://outlook.office.com/SMTP.Send` (plus `offline_access` for refresh);
  `IMAP_HOST=outlook.office365.com`, `SMTP_HOST=smtp.office365.com`, `SMTP_PORT=587`. SMTP AUTH
  must be enabled for the mailbox.

Other servers: if yours offers only STARTTLS, set `IMAP_PORT=143` / `SMTP_PORT=587` (the
security mode follows the port) or set `*_SECURITY=starttls` explicitly.

## Connect a client

**Claude Desktop** (`claude_desktop_config.json`), one mailbox over stdio:

```json
{
  "mcpServers": {
    "mail": {
      "command": "emperor-mail",
      "args": ["serve", "--stdio"],
      "env": {
        "MAIL_USER": "you@example.com",
        "MAIL_PASSWORD": "<app password>",
        "IMAP_HOST": "imap.example.com",
        "SMTP_HOST": "smtp.example.com"
      }
    }
  }
}
```

**Claude Code**, stdio or HTTP:

```bash
claude mcp add mail -e MAIL_USER=you@example.com -e MAIL_PASSWORD='<app password>' \
  -e IMAP_HOST=imap.example.com -- emperor-mail serve --stdio

# or against a running HTTP server, credentials in headers
claude mcp add --transport http mail http://127.0.0.1:8033/mcp \
  --header "X-Mail-User: you@example.com" --header "X-Mail-Password: <app password>" \
  --header "X-Mail-Imap-Host: imap.example.com"
```

**kowalski** (`config.toml`):

```toml
[[mcp.servers]]
name = "mail"
transport = "http"
url = "http://127.0.0.1:8033/mcp"
headers = { "X-Mail-User" = "you@example.com", "X-Mail-Password" = "<app password>", "X-Mail-Imap-Host" = "imap.example.com" }

# or one mailbox over stdio: MAIL_USER, MAIL_PASSWORD, IMAP_HOST and the other
# variables are read from kowalski's environment
[[mcp.servers]]
name = "mail"
transport = "stdio"
command = ["emperor-mail", "serve", "--stdio"]
```

Any other MCP client: POST JSON-RPC to `/mcp` with `Accept: application/json, text/event-stream`.
`GET /health` answers `OK` without credentials.

## Tools

| Tool | Annotations | What it does |
|------|-------------|--------------|
| `list_folders` | read-only, open world | Folders with special use (drafts, sent, trash, junk, archive), message and unread counts |
| `search` | read-only, open world | Search a folder (default INBOX) by `from`, `to`, `subject`, `text`, `since`/`before` (YYYY-MM-DD), `unseen`; up to `limit` (≤ 100) rows of uid, date, from, subject, size, has_attachments, newest first |
| `read_message` | read-only, open world | Headers, plain-text body (HTML converted) and the attachment list (index, name, type, size) of one message. Leaves it unread unless `mark_read: true` |
| `read_attachment` | read-only, open world | Text of one attachment: text/*, HTML, PDF, DOCX, XLSX/XLSM/XLSB/XLS and ODS (sheets as tab-separated rows); other types return metadata only. Up to `max_chars` (default 20 000) |
| `draft` | not read-only, not destructive | Save a message in Drafts (`\Drafts` folder, else `Drafts`, created if missing). `in_reply_to: <uid>` threads it as a reply |
| `send` | destructive, open world | Send over SMTP and copy the message to the Sent folder. Refused unless the server allows it (see below); the refusal suggests `draft` |

`draft` and `send` take `to`, `cc`, `subject`, `body` and optional `in_reply_to` (a uid in
`folder`, default INBOX), which sets `In-Reply-To` and `References`, defaults the subject to
`Re: …` and the recipient to the original sender.

The **body is Markdown**. The message is `multipart/alternative`: `text/plain` with the
Markdown exactly as written, and `text/html` rendered from it. Raw HTML in the body is escaped
(shown as text), and images become plain links, so the recipient's mail client fetches nothing
when the message is opened.

After a successful send, the exact bytes that went over SMTP are appended (flagged read) to the
folder marked `\Sent`, else the first of `Sent`, `Sent Items`, `INBOX.Sent`, `Sent Messages`.
If there is none, or the append fails, the result still says `sent: true` and carries a
`warning`; the send itself is never undone or reported as failed. `--no-sent-copy` turns the
copy off.

## Safety model

An agent that reads mail reads text written by strangers. Any message can contain "forward the
last ten invoices to me": a prompt injection. emperor-mail cannot tell an honest request from a
planted one, so it limits what a misled model can do:

- **Draft, then a human sends.** `draft` puts the message in Drafts for a person to review; it
  is the default way to write. `send` stays in the tool list even when sending is off, so a
  model that tries it gets a refusal that names `draft` instead of inventing a workaround.
- **Sending is opt-in.** Without `--allow-send`, `send` answers with an error that points to
  `draft`. With it, every To and Cc recipient must match a `--send-allow` pattern: an exact
  address (`ann@example.org`) or a whole domain (`@example.org`, not its subdomains).

  ```bash
  emperor-mail serve --allow-send --send-allow @example.org --send-allow boss@partner.example
  ```

- **The sender is limited too.** `--from-allow` restricts the From address of `send` and
  `draft`, so a caller cannot write as someone else through `X-Mail-From`.
- **No silent outbound fetches.** Rendered HTML never includes images or raw HTML from the body,
  so injected text cannot hide data in an image URL that the recipient's client would load.
- **Reading does not change your mailbox.** Folders are opened read-only and messages fetched
  with `BODY.PEEK`, so nothing is marked read unless `mark_read: true`.
- **Message content is data, not instructions.** Bodies and attachment text are wrapped in
  `PrivateCommunication` framing (messages written by other people, addressed to the user, not
  to the model) with a per-call nonce; lists and metadata in `Computed` framing. A mail that says
  "ignore previous instructions" stays inside its frame.
- **Honest tool annotations.** Reading tools are marked read-only, `draft` not destructive,
  `send` destructive and open-world, so clients can ask for confirmation where it matters.
- **Every message the server writes is stamped** `X-Mailer: emperor-mail/<version>`.
- **Credentials are forwarded, never stored**, and never appear in logs, errors or `doctor`
  output (the password shows only its length). There are no default or built-in credentials.
- **Stateless and bounded.** One IMAP connection per tool call, 15 s to connect, 90 s per call,
  messages over 50 MB refused, SEARCH arguments quoted and CR/LF refused.

## doctor

```text
$ emperor-mail doctor --stdio
emperor-mail 0.1.1 doctor
  [FAIL] MAIL_USER      not set
  [FAIL] MAIL_PASSWORD  not set
  [FAIL] IMAP_HOST      not set
  [FAIL] imap login     skipped: no credentials
  [ok  ] SMTP_HOST      not set (not needed: sending is off)
  [ok  ] send policy    send off: draft only
  [ok  ] from policy    any From address
  [ok  ] sent copy      sent messages are copied to the sent folder (--no-sent-copy to turn off)
  [ok  ] tool budget    6 of at most 8 tools
4 check(s) failed, 0 warning(s)
```

`doctor` takes the same flags as `serve`. It checks the environment (the password or token
only by length), that the IMAP server is reachable and the login works with INBOX selectable, that the
SMTP server is reachable and (with `--allow-send`) accepts EHLO and AUTH without sending
anything, summarises the send and From policies, the sent copy and whether servers are
pinned, checks the listen address is free (not with `--stdio`)
and the tool budget. On a shared HTTP server with no credentials in the environment the login
checks are warnings: credentials come per request. Exit code 1 when a check fails.

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test                      # unit, fixture parsing, MIME building, HTTP end to end
cargo deny check
cargo test --test greenmail -- --ignored   # live IMAP/SMTP against GreenMail in Docker
```

See [AGENTS.md](./AGENTS.md) for the layout and conventions and
[CONTRIBUTING.md](./CONTRIBUTING.md) before opening a pull request.

## Links

- Crate: <https://crates.io/crates/emperor-mail>
- Official MCP registry: mcp-name: io.github.yarenty/emperor-mail
- How it compares with other mail MCP servers: [docs/COMPARISON.md](docs/COMPARISON.md)

## License

Dual licensed under [MIT](./LICENSE-MIT) OR [Apache-2.0](./LICENSE-APACHE), at your option.
