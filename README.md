# emperor-mail

[![CI](https://github.com/yarenty/emperor-mail/actions/workflows/ci.yml/badge.svg)](https://github.com/yarenty/emperor-mail/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**Your mailbox as six MCP tools: search, read, draft, and send only when you allow it.**

emperor-mail is an [MCP](https://modelcontextprotocol.io) server that reads and writes email
over IMAP and SMTP, so an agent can find a message, read it and its attachments (PDFs
included), and draft a reply for you to send.

- Built on [emperor-mcp](https://github.com/yarenty/emperor-mcp): stateless Streamable HTTP,
  framed tool output, forwarded credentials (Emperor Profile P1).
- Two ways to run: **one mailbox** (credentials in the environment, stdio or HTTP) or a
  **shared server** (each request brings its own credentials in headers; nothing is stored).
- **Draft by default.** `send` is off unless you start the server with `--allow-send`, and even
  then only to the recipients you list.
- Single binary, pure Rust: IMAP via [async-imap](https://github.com/async-email/async-imap),
  parsing via [mail-parser](https://github.com/stalwartlabs/mail-parser), SMTP via
  [lettre](https://github.com/lettre/lettre), PDF text via [lopdf](https://github.com/J-F-Liu/lopdf),
  TLS via rustls and the operating system's trust store.

## Install

Prebuilt binaries (macOS arm64/x86_64, Linux x86_64, Windows x86_64) are attached to
[GitHub releases](https://github.com/yarenty/emperor-mail/releases). From source:

```bash
cargo install --git https://github.com/yarenty/emperor-mail
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
| `MAIL_FROM` | `From` address when the login name is not an address | the login name |
| `IMAP_HOST` / `IMAP_PORT` | IMAP server | port 993 (143 with `starttls`) |
| `IMAP_SECURITY` | `tls`, `starttls` or `plain` | `tls`, or `starttls` on port 143 |
| `SMTP_HOST` / `SMTP_PORT` | SMTP server | port 465 (587 with `starttls`) |
| `SMTP_SECURITY` | `tls`, `starttls` or `plain` | `tls`, or `starttls` on ports 587 and 25 |

`plain` (no encryption) is accepted only for a loopback host, such as a local mail bridge.
Certificates are always verified against the operating system's trust store.

## Run as a shared server

On a shared server every request carries the caller's own credentials in headers. They are
used for that one tool call and dropped with the request: never stored, never logged.

```bash
emperor-mail serve --bind 127.0.0.1:8033
```

| Header | Same as |
|--------|---------|
| `X-Mail-User`, `X-Mail-Password` | `MAIL_USER`, `MAIL_PASSWORD` (always as a pair) |
| `X-Mail-From` | `MAIL_FROM` |
| `X-Imap-Host`, `X-Imap-Port`, `X-Imap-Security` | `IMAP_*` |
| `X-Smtp-Host`, `X-Smtp-Port`, `X-Smtp-Security` | `SMTP_*` |

Rules:

- If a request carries `X-Mail-User` or `X-Mail-Password`, both must be there; a header user is
  never combined with a password from the environment.
- A host set in the environment **pins** that server: `IMAP_HOST=imap.corp.example` means every
  caller logs in there and `X-Imap-*` headers are ignored. Leave it unset to let callers name
  their own server.
- Put the server behind an ingress that terminates TLS and authenticates callers; the headers
  carry passwords. Without one, keep it on `127.0.0.1`.

## Provider notes

- **Gmail**: turn on 2-Step Verification, then create an app password at
  <https://myaccount.google.com/apppasswords>. `IMAP_HOST=imap.gmail.com`,
  `SMTP_HOST=smtp.gmail.com`. IMAP must be enabled in Gmail settings for some accounts. Gmail
  files sent mail in "[Gmail]/Sent Mail" by itself.
- **Fastmail**: create an app password under Settings → Privacy & Security → Integrations →
  App passwords, with IMAP and SMTP access. `IMAP_HOST=imap.fastmail.com`,
  `SMTP_HOST=smtp.fastmail.com`.
- **Outlook / Microsoft 365** accounts generally require OAuth for IMAP; password login is not
  supported there.

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
  --header "X-Imap-Host: imap.example.com"
```

**kowalski** (`config.toml`):

```toml
[[mcp.servers]]
name = "mail"
transport = "http"
url = "http://127.0.0.1:8033/mcp"
headers = { "X-Mail-User" = "you@example.com", "X-Mail-Password" = "<app password>", "X-Imap-Host" = "imap.example.com" }
```

Any other MCP client: POST JSON-RPC to `/mcp` with `Accept: application/json, text/event-stream`.
`GET /health` answers `OK` without credentials.

## Tools

| Tool | What it does |
|------|--------------|
| `list_folders` | Folders with special use (drafts, sent, trash, junk, archive), message and unread counts |
| `search` | Search a folder (default INBOX) by `from`, `to`, `subject`, `text`, `since`/`before` (YYYY-MM-DD), `unseen`; up to `limit` (≤ 100) rows of uid, date, from, subject, size, has_attachments, newest first |
| `read_message` | Headers, plain-text body (HTML converted) and the attachment list (index, name, type, size) of one message. Leaves it unread unless `mark_read: true` |
| `read_attachment` | Text of one attachment: text/*, HTML and PDF; other types return metadata only. Up to `max_chars` (default 20 000) |
| `draft` | Save a message in Drafts (`\Drafts` folder, else `Drafts`, created if missing). Always allowed. `in_reply_to: <uid>` threads it as a reply |
| `send` | Send over SMTP. Refused unless the server allows it (see below); the refusal suggests `draft` |

`draft` and `send` take `to`, `cc`, `subject`, `body` (plain text or Markdown, sent as UTF-8
text) and optional `in_reply_to` (a uid in `folder`, default INBOX), which sets `In-Reply-To`
and `References`, defaults the subject to `Re: …` and the recipient to the original sender.

## Safety model

- **Sending is opt-in.** Without `--allow-send`, `send` answers with an error that points to
  `draft`. With it, every To and Cc recipient must match a `--send-allow` pattern: an exact
  address (`ann@example.org`) or a whole domain (`@example.org`, not its subdomains).

  ```bash
  emperor-mail serve --allow-send --send-allow @example.org --send-allow boss@partner.example
  ```

- **Reading does not change your mailbox.** Folders are opened read-only and messages fetched
  with `BODY.PEEK`, so nothing is marked read unless `mark_read: true`.
- **Message content is data, not instructions.** Bodies and attachment text are wrapped in
  `ExternalWeb` framing with a per-call nonce; lists and metadata in `Computed` framing. A mail
  that says "ignore previous instructions" stays inside its frame.
- **Every message the server writes is stamped** `X-Mailer: emperor-mail/<version>`.
- **Credentials are forwarded, never stored**, and never appear in logs, errors or `doctor`
  output (the password shows only its length). There are no default or built-in credentials.
- **Stateless and bounded.** One IMAP connection per tool call, 15 s to connect, 90 s per call,
  messages over 50 MB refused, SEARCH arguments quoted and CR/LF refused.

## doctor

```text
$ emperor-mail doctor --stdio
emperor-mail 0.1.0 doctor
  [FAIL] MAIL_USER      not set
  [FAIL] MAIL_PASSWORD  not set
  [FAIL] IMAP_HOST      not set
  [FAIL] imap login     skipped: no credentials
  [ok  ] SMTP_HOST      not set (not needed: sending is off)
  [ok  ] send policy    send off: draft only
  [ok  ] tool budget    6 of at most 8 tools
4 check(s) failed, 0 warning(s)
```

`doctor` takes the same flags as `serve`. It checks the environment (the password only by
length), that the IMAP server is reachable and the login works with INBOX selectable, that the
SMTP server is reachable and (with `--allow-send`) accepts EHLO and AUTH without sending
anything, summarises the send policy, checks the listen address is free (not with `--stdio`)
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

## License

Dual licensed under [MIT](./LICENSE-MIT) OR [Apache-2.0](./LICENSE-APACHE), at your option.
