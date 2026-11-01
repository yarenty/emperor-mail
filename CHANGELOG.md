# Changelog

All notable changes to emperor-mail. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [semantic versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-01

> First release: a mailbox as six MCP tools, draft by default.

### Added
- **Six tools** - `list_folders`, `search`, `read_message`, `read_attachment`, `draft`, `send`, over IMAP and SMTP.
- **Two ways to run** - one mailbox from the environment (`serve --stdio` for desktop clients, or HTTP), or a shared Streamable HTTP server where each request brings its own credentials in `X-Mail-*` / `X-Imap-*` / `X-Smtp-*` headers, used for that call only and never stored or logged. A host set in the environment pins the server for every caller.
- **Send guard** - `send` is off unless the server runs with `--allow-send`, and then only to recipients matching `--send-allow` (an address or `@domain`); otherwise the error points to `draft`. `draft` saves to the `\Drafts` folder (or `Drafts`, created if missing) and threads replies with In-Reply-To and References.
- **Attachments** - text of text/*, HTML and PDF attachments (pure-Rust PDF extraction); other types are listed with name, type and size.
- **Reading leaves mail unread** - `BODY.PEEK` unless `mark_read: true`.
- **Framed output** - bodies and attachment text framed as external content, lists and metadata as computed data (Emperor Profile P1).
- `X-Mailer: emperor-mail/0.1.0` on every message the server drafts or sends.
- **`doctor`** - environment, IMAP reachability and login, SMTP reachability and (with `--allow-send`) EHLO/AUTH without sending, send policy, listen address, tool budget.
- TLS through rustls and the operating system's trust store; STARTTLS supported; unencrypted connections only to loopback hosts.
- Prebuilt binaries for macOS (arm64, x86_64), Linux x86_64 and Windows x86_64 on each tagged release.

[Unreleased]: https://github.com/yarenty/emperor-mail/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/yarenty/emperor-mail/releases/tag/v0.1.0
