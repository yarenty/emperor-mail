# Changelog

All notable changes to emperor-mail. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [semantic versioning](https://semver.org/).

## [Unreleased]

## [0.1.3] - 2026-11-01

> Read-only by default: writing is switched on by the operator.

### Added
- **`--writable` / `MAIL_WRITABLE`** - lists `draft`. Without it the server is read-only and lists four tools: `list_folders`, `search`, `read_message`, `read_attachment`.
- **Mode in the start-up log and `doctor`** - `read-only`, `drafts`, or `drafts+send with N recipient pattern(s)`.

### Changed
- **`draft` now needs `--writable`** (or `MAIL_WRITABLE=true`). Saving to Drafts changes the mailbox, so it is a write and is off by default. Add `--writable` to keep drafting as before.
- **`send` is listed only with `--allow-send`** (or `MAIL_ALLOW_SEND=true`), which also lists `draft`. Before, `send` was always listed and refused when sending was off. The recipient rules are unchanged: with sending on and no `--send-allow` pattern, every recipient is refused.
- **Switch values are checked** - `MAIL_WRITABLE` and `MAIL_ALLOW_SEND` accept `true`/`1`/`yes`/`on` and `false`/`0`/`no`/`off` (or empty); anything else stops the server at start-up instead of being read as on.
- **Smaller tool definitions** - compact input schemas (no `$schema`, no `null` alternatives on optional arguments, no integer formats). `tools/list` is 2.8 KB read-only, 4.0 KB with `--writable` and 5.1 KB with `--allow-send`, down from 6.1 KB.
- **emperor-mcp 0.4** and Emperor Profile P1.1.
- **Registry metadata** - `server.json` adds `--writable` and `MAIL_WRITABLE`.

## [0.1.2] - 2026-11-01

### Added
- **Send switches from the environment** - `MAIL_ALLOW_SEND` (`--allow-send`; unset, empty, `false`, `0` or `no` mean off), `MAIL_SEND_ALLOW` (`--send-allow`) and `MAIL_FROM_ALLOW` (`--from-allow`), comma-separated, for the Docker image and catalogs that can only set variables. With sending on and no recipient pattern every recipient is still refused.
- **Docker image check on amd64 and arm64** - CI builds the image on both architectures and runs it over stdio against a GreenMail server.

### Changed
- **Send policy summary** - the start-up log and `doctor` say whether send is on and how many recipient patterns there are.
- **Registry metadata** - `server.json` lists `--stdio` and `--allow-send` as fixed positional arguments, adds `--from-allow` and the three new variables.

## [0.1.1] - 2026-11-01

### Added
- **Registry metadata** - `server.json` for the official MCP registry (`io.github.yarenty/emperor-mail`, cargo package, stdio, every environment variable described), and the registry name in the README.
- **Dockerfile** - a multi-stage image (Debian slim runtime with the CA bundle, non-root user, OCI labels) that runs `emperor-mail serve --stdio` with one mailbox from the environment.

### Fixed
- **kowalski snippet in the README** - the stdio entry now uses `command` as an argument list (`["emperor-mail", "serve", "--stdio"]`); the mail variables come from kowalski's environment.

## [0.1.0] - 2026-11-01

> First release: a mailbox as six MCP tools, draft by default.

### Added
- **Six tools** - `list_folders`, `search`, `read_message`, `read_attachment`, `draft`, `send`, over IMAP and SMTP, each with MCP annotations: the four reading tools read-only and open-world, `draft` not destructive, `send` destructive and open-world.
- **Two ways to run** - one mailbox from the environment (`serve --stdio` for desktop clients, or HTTP), or a shared Streamable HTTP server where each request brings its own credentials in `X-Mail-*` headers, used for that call only and never stored or logged.
- **Servers from headers** - a caller with its own login may name its IMAP and SMTP servers (`X-Mail-Imap-Host`, `-Port`, `-Security`, and the `X-Mail-Smtp-*` set), falling back to the environment; always over TLS or STARTTLS. With a login from the environment, server headers are ignored. `--pin-servers` makes the environment's servers win for everyone.
- **OAuth 2.0** - XOAUTH2 for IMAP and SMTP with an access token from `X-Mail-Oauth-Token` or `MAIL_OAUTH_TOKEN` (Gmail, Microsoft 365); the client obtains the token, the server only forwards it.
- **401 on rejected credentials** - when the mail server refuses the login or token, the HTTP response is `401` with a JSON-RPC error, so the client can ask the user to sign in again.
- **Send guard** - `send` is off unless the server runs with `--allow-send`, and then only to recipients matching `--send-allow` (an address or `@domain`); otherwise the error points to `draft`. `send` stays listed when sending is off so the refusal can steer to `draft`. `draft` saves to the `\Drafts` folder (or `Drafts`, created if missing) and threads replies with In-Reply-To and References.
- **Sender allow-list** - `--from-allow` limits the From address of `send` and `draft`.
- **Sent folder copy** - each sent message is appended to the Sent folder (`\Sent`, else `Sent`, `Sent Items`, `INBOX.Sent`, `Sent Messages`); a missing folder is a warning, not a failure. `--no-sent-copy` turns it off.
- **Markdown bodies** - `draft` and `send` write `multipart/alternative`: the Markdown as `text/plain` and rendered `text/html`, with raw HTML escaped and images turned into links.
- **Attachments** - text of text/*, HTML, PDF, Word (DOCX) and spreadsheet (XLSX, XLSM, XLSB, XLS, ODS) attachments, all pure Rust; other types are listed with name, type and size.
- **Reading leaves mail unread** - `BODY.PEEK` unless `mark_read: true`.
- **Framed output** - bodies and attachment text framed as private communication, lists and metadata as computed data (Emperor Profile P1).
- `X-Mailer: emperor-mail/0.1.0` on every message the server drafts or sends.
- **`doctor`** - environment, IMAP reachability and login, SMTP reachability and (with `--allow-send`) EHLO/AUTH without sending, send and From policies, sent copy, server pinning, listen address, tool budget.
- TLS through rustls and the operating system's trust store; STARTTLS supported; unencrypted connections only to loopback hosts named in the environment.
- Prebuilt binaries for macOS (arm64, x86_64), Linux x86_64 and Windows x86_64 on each tagged release, with build provenance attestations and a `SHA256SUMS` file.

[Unreleased]: https://github.com/yarenty/emperor-mail/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/yarenty/emperor-mail/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/yarenty/emperor-mail/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/yarenty/emperor-mail/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/yarenty/emperor-mail/releases/tag/v0.1.0
