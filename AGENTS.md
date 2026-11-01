# emperor-mail — AI agent notes

**Crate**: `emperor-mail` · **Version**: 0.1.0 (not yet on crates.io) · **Licence**: MIT OR Apache-2.0 · **Rust**: 1.88 (`rust-version`; edition 2024, let-chains)

## Scope

An **MCP server for email** (IMAP + SMTP) built on [`emperor-mcp`](https://github.com/yarenty/emperor-mcp) 0.2 with an rmcp 3.8 `#[tool_router]` server: stateless Streamable HTTP at `/mcp` + `/health` through `emperor_mcp::serve`, or stdio (`serve --stdio`) through rmcp's `transport-io`. Exactly **six tools** (`list_folders`, `search`, `read_message`, `read_attachment`, `draft`, `send`), inside the framework's 3–8 budget. emperor-mcp comes from crates.io.

Layout (one crate, binary + library so tests can drive it):

- `src/main.rs` — clap CLI: `serve` and `doctor`, both taking `--bind`, `--allow-send`, `--send-allow`, `--stdio`.
- `src/server.rs` — `MailServer` and the six tools. Thin: resolve credentials, one IMAP/SMTP call, frame the result.
- `src/config.rs` — `Settings` (one source: env or headers), `Credentials::resolve` (the only place the two meet), `Endpoint`/`Security`, `HEADER_NAMES` and `forward_config()`, `Secret`.
- `src/imap.rs` — connect (TLS / STARTTLS / loopback-only plain), `with_session` (one connection per call, timeouts), list/search/fetch/append.
- `src/smtp.rs` — lettre transport per send, `check` for doctor.
- `src/message.rs` — mail-parser: headers, body (HTML → text), attachment list, attachment text (PDF on the blocking pool).
- `src/compose.rs` — lettre `Message` for draft and send; `X-Mailer`, reply threading.
- `src/search.rs` — tool arguments → IMAP `SEARCH` keys (quoting, CR/LF refusal, dates).
- `src/policy.rs` — `SendPolicy` (`--allow-send`, `--send-allow` patterns).
- `src/doctor.rs` — the `doctor` report on `emperor_mcp::doctor`.

Tests: unit tests in each module; `tests/parsing.rs` (fixture `.eml` files in `tests/fixtures/`), `tests/compose.rs` (MIME out, parsed back), `tests/http.rs` (real HTTP through `emperor_mcp::serve`, a scripted IMAP server on loopback records what the tool sent), `tests/greenmail.rs` (`#[ignore]`: live IMAP/SMTP against a GreenMail container the test starts and removes).

## Before you change code

1. Read `src/config.rs` (credential rules) and `src/server.rs` (tool flow).
2. Run **`cargo fmt --all --check`**, **`cargo clippy --all-targets -- -D warnings`**, **`cargo test`** and **`cargo deny check`**. When you touch IMAP, SMTP or drafts, also run **`cargo test --test greenmail -- --ignored`** (needs Docker).

## Conventions

- **One `Credentials`, two sources.** Environment and forwarded headers both become `Settings`; `Credentials::resolve` is the only merge point. User and password always come as a pair from one source; an endpoint set in the environment pins the server and header endpoints are ignored. Never add a third path, a default credential or a "test mode" (Emperor Profile E7). New header names go into `Field` and `HEADER_NAMES` together (a unit test checks they agree).
- **Credentials are never stored or logged** (E6). `Secret` has no `Display` and a redacted `Debug`; errors name the user, never the password; doctor shows the password's length only. Do not log tool arguments wholesale (they may contain mail content).
- **Stateless.** One IMAP connection per tool call via `imap::with_session` (connect timeout, whole-call timeout, logout). No connection pool, no cache across calls, nothing per user in `MailServer`.
- **Reading never changes the mailbox** unless asked: `EXAMINE` + `BODY.PEEK[...]`; only `mark_read: true` uses `SELECT` + `BODY[]`.
- **Sending is guarded.** `send` checks `SendPolicy` before resolving credentials when sending is off, and on the final recipient list (after reply defaults) when it is on. `draft` is never restricted. Every outgoing message goes through `compose::build`, which adds `X-Mailer`.
- **Framing** (E8): bodies and attachment text `FrameKind::ExternalWeb` (anyone can send mail); lists and metadata JSON `FrameKind::Computed` (`structured_framed`). Message bodies never go into `structured_content` unframed. Errors are `isError: true` results with a plain `error: …` line.
- **Untrusted input to IMAP** goes through `search::quote` (escapes `\` and `"`, refuses CR/LF/NUL). Mailbox names go through async-imap's own quoting. New SEARCH keys get a test in `src/search.rs`, including an injection case.
- **CPU-bound work** (PDF extraction) runs in `spawn_blocking`; a panic in the parser becomes a tool error.
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
| HTML | `html2text` 0.17 | Maintained (regular releases), html5ever parser, drops `<script>`/`<style>`, keeps links as footnotes. A hand-written tag stripper gets entities, `<br>`, tables and scripts wrong; this one is only a fallback when html2text fails. |

## Documentation closure (mandatory)

After any behaviour change, update **[`README.md`](./README.md)** (tools table, safety model, doctor) and **[`manifest.yaml`](./manifest.yaml)** (tool list); add a [`CHANGELOG.md`](./CHANGELOG.md) entry under `[Unreleased]` for anything a user would notice. **Shipping code without updating docs is incomplete work.**

## Related docs

- [`README.md`](./README.md) — install, run, connect, tools, safety model
- [`CHANGELOG.md`](./CHANGELOG.md) — what each release changed
- [`manifest.yaml`](./manifest.yaml) — MCP manifest (emperor-mcp `MANIFEST_SPEC.md`)
- [`CONTRIBUTING.md`](./CONTRIBUTING.md) — checks and DCO sign-off
