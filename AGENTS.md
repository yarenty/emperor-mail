# emperor-mail — AI agent notes

**Crate**: `emperor-mail` · **Version**: 0.1.3 · **Licence**: MIT OR Apache-2.0 · **Rust**: 1.88 (`rust-version`; edition 2024, let-chains)

## Scope

An **MCP server for email** (IMAP + SMTP) built on [`emperor-mcp`](https://github.com/yarenty/emperor-mcp) 0.4 (Emperor Profile P1.1) with an rmcp 3.8 `#[tool_router]` server: stateless Streamable HTTP at `/mcp` + `/health` through `emperor_mcp::serve`, or stdio (`serve --stdio`) through rmcp's `transport-io`. Four reading tools (`list_folders`, `search`, `read_message`, `read_attachment`) by default; `--writable` adds `draft`, `--allow-send` adds `send` (and `draft`): at most **six tools**, inside the framework's 3–8 budget. emperor-mcp comes from crates.io.

Layout (one crate, binary + library so tests can drive it):

- `src/main.rs` — clap CLI: `serve` and `doctor`, both taking `--bind`, `--writable`, `--allow-send`, `--send-allow`, `--from-allow`, `--no-sent-copy`, `--pin-servers`, `--stdio`. The write switches also read `MAIL_WRITABLE` and `MAIL_ALLOW_SEND` (`emperor_mcp::cli::parse_switch`: falsey is off, a typo is refused), `MAIL_SEND_ALLOW` and `MAIL_FROM_ALLOW` (comma-separated) for the Docker image.
- `src/server.rs` — `MailServer`, `Options` and the six tools with their annotations; `MailServer::router(mode)` builds the compact router and drops the write tools the mode does not allow. Thin: resolve credentials, one IMAP/SMTP call, frame the result. `done` turns `MailError::AuthRejected` into `emperor_mcp::credentials_rejected` (HTTP 401), every other error into an `isError` result.
- `src/config.rs` — `Settings` (one source: env or headers), `Credentials::resolve` (the only place the two meet), `Auth` (password or OAuth token), `xoauth2_initial_response`, `Endpoint`/`Security`, `HEADER_NAMES` and `forward_config()`, `Secret`.
- `src/imap.rs` — connect (TLS / STARTTLS / loopback-only plain), `LOGIN` or `AUTHENTICATE XOAUTH2`, `with_session` (one connection per call, timeouts), list/search/fetch/append, drafts and sent folder lookup.
- `src/smtp.rs` — lettre transport per send (PLAIN/LOGIN or XOAUTH2), `send` of the exact bytes, `check` for doctor.
- `src/message.rs` — mail-parser: headers, body (HTML → text), attachment list, attachment text (PDF, DOCX and spreadsheets on the blocking pool).
- `src/compose.rs` — lettre `Message` for draft and send: Markdown body as `multipart/alternative` (text as written + HTML from pulldown-cmark), `X-Mailer`, reply threading.
- `src/search.rs` — tool arguments → IMAP `SEARCH` keys (quoting, CR/LF refusal, dates).
- `src/policy.rs` — `Mode` (read-only, drafts, drafts+send: which tools are listed), `SendPolicy` (`--allow-send`, `--send-allow`) and `FromPolicy` (`--from-allow`), one pattern syntax.
- `src/doctor.rs` — the `doctor` report on `emperor_mcp::doctor`.

Tests: unit tests in each module; `tests/parsing.rs` (fixture `.eml` files in `tests/fixtures/`), `tests/compose.rs` (MIME out, parsed back), `tests/http.rs` (real HTTP through `emperor_mcp::serve`: the tool list in each mode (4 / 5 / 6), `testkit::assert_read_only_default` on the default list, `assert_compact_schemas` and `assert_tools` on a live `tools/list`; a scripted IMAP server on loopback records what the tool sent, including LOGIN, XOAUTH2 and the 401 on refused credentials), `tests/greenmail.rs` (`#[ignore]`: live IMAP/SMTP against a GreenMail container the test starts and removes, including the Sent copy; reads on a server with no switch, drafts with `--writable`, sends with `--allow-send`). `scripts/docker-greenmail.sh` runs the built image over stdio against GreenMail (doctor, the write and send variables switching tools on and off, initialize/tools/list/list_folders with stdout checked to be JSON-RPC only); the `Docker` workflow runs it on amd64 and arm64.

## Before you change code

1. Read `src/config.rs` (credential rules) and `src/server.rs` (tool flow).
2. Run **`cargo fmt --all --check`**, **`cargo clippy --all-targets -- -D warnings`**, **`cargo test`** and **`cargo deny check`**. When you touch IMAP, SMTP or drafts, also run **`cargo test --test greenmail -- --ignored`** (needs Docker).

## Conventions

- **One `Credentials`, two sources.** Environment and forwarded headers both become `Settings`; `Credentials::resolve` is the only merge point. The login (user plus exactly one of password and OAuth token) comes whole from one source. Endpoints follow the login: a header login may name its servers in headers (falling back to the environment); an environment login never uses header endpoints; `--pin-servers` makes the environment win for everyone. A header endpoint is never `plain`. Never add a third path, a default credential or a "test mode" (Emperor Profile E7). New header names go into `Field` and `HEADER_NAMES` together, all `X-Mail-*` (a unit test checks they agree).
- **Credentials are never stored or logged** (E6). `Secret` (password and token) has no `Display` and a redacted `Debug`; errors name the user, never the secret; doctor shows the secret's length only. The server never obtains or refreshes OAuth tokens; the client does.
- **Refused credentials are a 401.** IMAP `NO` to LOGIN/AUTHENTICATE and SMTP 534/535 become `MailError::AuthRejected`, which `server::done` returns as `credentials_rejected` (JSON-RPC `-32040`, HTTP 401 through `emperor_mcp::serve`). Any other failure stays an `isError` result. Do not log tool arguments wholesale (they may contain mail content).
- **Stateless.** One IMAP connection per tool call via `imap::with_session` (connect timeout, whole-call timeout, logout). No connection pool, no cache across calls, nothing per user in `MailServer`.
- **Reading never changes the mailbox** unless asked: `EXAMINE` + `BODY.PEEK[...]`; only `mark_read: true` uses `SELECT` + `BODY[]`.
- **Writes are opt-in (Emperor Profile E11).** With no switch only the four reading tools are listed; `--writable` / `MAIL_WRITABLE` adds `draft` (saving to Drafts changes the mailbox); `--allow-send` / `MAIL_ALLOW_SEND` adds `send` and implies `--writable`. A tool the mode does not allow is removed from the router, not listed-and-refusing: a listed tool invites the attempt, from the model or from an instruction planted in a message. `draft` and `send` still refuse in their handlers when the mode does not allow them (defence in depth). The start-up log and `doctor` state the mode. Never add an off-switch (`--read-only`) instead: forgetting it must leave the server unable to write.
- **Sending is guarded.** `send` checks `SendPolicy` on the final recipient list (after reply defaults); with sending on and no pattern every recipient is refused. `draft` is never restricted by recipient. Both check `FromPolicy` on the From address. Every outgoing message goes through `compose::build`, which adds `X-Mailer`.
- **Sent copy.** After a successful SMTP send the exact bytes sent are appended (`\Seen`) to the `\Sent` folder, else the first of `imap::SENT_NAMES`. A missing folder or failed append is a `warning` in the result, never a send failure. `--no-sent-copy` turns it off (Gmail, Exchange Online file sent mail themselves).
- **Outgoing HTML** comes only from `compose::markdown_to_html`: raw HTML events are escaped as text and images become links. Never pass HTML through and never emit `<img>`: an injected instruction could hide data in an image URL the recipient's client would fetch.
- **Lean definitions (E12).** The router is built with `.compact_schemas()`; tool descriptions stay under 300 characters (a test fails on any `testkit::warnings`), parameter descriptions one line.
- **Annotations** (checked by `emperor_mcp::testkit` in `tests/http.rs`): reading tools `readOnlyHint: true, openWorldHint: true`; `draft` not read-only, not destructive; `send` destructive and open-world. A changed or new tool keeps them explicit.
- **Framing** (E8): bodies and attachment text `FrameKind::PrivateCommunication` (written by other people, addressed to the user, may carry instructions); lists and metadata JSON `FrameKind::Computed` (`structured_framed`). Message bodies never go into `structured_content` unframed. Errors are `isError: true` results with a plain `error: …` line.
- **Untrusted input to IMAP** goes through `search::quote` (escapes `\` and `"`, refuses CR/LF/NUL). Mailbox names go through async-imap's own quoting. New SEARCH keys get a test in `src/search.rs`, including an injection case.
- **CPU-bound work** (PDF, DOCX, spreadsheet extraction) runs in `spawn_blocking`; a panic in a parser becomes a tool error. Office files are size-capped before opening and `word/document.xml` is read through a byte limit.
- **TLS** is rustls with the ring provider and `rustls-platform-verifier` (OS trust store). There is no "accept invalid certificates" option and there must not be one; `plain` is for loopback only.
- **Tool budget**: six. A new capability that needs a seventh or more tools is a separate server (split by noun), not a bigger one.
- Licence is dual `MIT OR Apache-2.0`; keep `Cargo.toml`, the README badge and licence section in agreement. Contributions are signed off (DCO, see `CONTRIBUTING.md`).

## Dependency choices

| Need | Crate | Why |
|------|-------|-----|
| IMAP | `async-imap` 0.12 (async-email org) | Maintained, used by Delta Chat; tokio runtime feature; exposes `BODYSTRUCTURE`, special-use attributes, `APPEND`. |
| TLS | `tokio-rustls` + `rustls-platform-verifier` | Pure Rust, OS trust store (same verifier lettre uses), no OpenSSL. |
| Parsing | `mail-parser` 0.11 (Stalwart Labs) | Robust MIME, RFC 2047 words, legacy charsets (ISO-8859-x, Windows-125x) without extra features. 1.0 needs Rust 1.98; move when the MSRV does. |
| SMTP | `lettre` 0.11 (no pool) | Builder for MIME + In-Reply-To/References; implicit TLS and STARTTLS; rustls with the platform verifier. |
| PDF | `lopdf` 0.46 (no default features) | Pure Rust text extraction with a decompression limit. |
| Markdown → HTML | `pulldown-cmark` 0.13 (no default features, `html`) | The reference CommonMark parser in Rust; events let us escape raw HTML and turn images into links. |
| DOCX | `zip` + `quick-xml` (both already under calamine) | Paragraph text from `word/document.xml` in ~50 lines; no extra crate in the tree. |
| Spreadsheets | `calamine` 0.37 (no default features) | Widely used, pure Rust; XLSX, XLSM, XLSB, XLS and ODS through one API; MSRV 1.88. |
| HTML | `html2text` 0.17 | Maintained (regular releases), html5ever parser, drops `<script>`/`<style>`, keeps links as footnotes. A hand-written tag stripper gets entities, `<br>`, tables and scripts wrong; this one is only a fallback when html2text fails. |

## Documentation closure (mandatory)

After any behaviour change, update **[`README.md`](./README.md)** (tools table, safety model, doctor) and **[`manifest.yaml`](./manifest.yaml)** (tool list); add a [`CHANGELOG.md`](./CHANGELOG.md) entry under `[Unreleased]` for anything a user would notice. **Shipping code without updating docs is incomplete work.**

## Related docs

- [`README.md`](./README.md) — install, run, connect, tools, safety model
- [`CHANGELOG.md`](./CHANGELOG.md) — what each release changed
- [`manifest.yaml`](./manifest.yaml) — MCP manifest (emperor-mcp `MANIFEST_SPEC.md`)
- [`CONTRIBUTING.md`](./CONTRIBUTING.md) — checks and DCO sign-off
- [`server.json`](./server.json) — official MCP registry entry; its two `version` fields and the Dockerfile's `VERSION` follow `Cargo.toml`
- [`Dockerfile`](./Dockerfile) — stdio image (builds `src/` only, see `.dockerignore`)
