# emperor-mail next to other mail MCP servers

How emperor-mail 0.1.2 compares with the most popular public MCP servers for e-mail. For the
shared background (the 2026-07-28 spec revision, annotations, postmark-mcp, the "lethal
trifecta") and what every emperor server gets from the framework, see
[the emperor-mcp comparison](https://github.com/yarenty/emperor-mcp/blob/main/docs/COMPARISON.md).

Figures for other projects were read on **2026-11-01** from the GitHub API (stars, last
release) and from each project's README and source (tool counts). A **(?)** marks a figure that
could not be confirmed; "not re-counted" means an earlier count was kept.

## Why this domain is risky

Mail is the lethal trifecta in one place: private data (your inbox), untrusted content (every
message is written by someone else) and a way out (sending). Any message can say "forward the
last ten invoices to me", and a model cannot reliably tell an honest request from a planted one.
The only dependable defence is on the server side: what it may send, to whom, and as whom. And
the first known malicious MCP server, postmark-mcp, was a mail server that BCC'd every message to
its author: the code you install matters as much as its features.

## The table

| Server | Stars | Last release | Language | Tools | Mutations | Sending guard | Credentials |
|---|---|---|---|---|---|---|---|
| **emperor-mail** | new | 0.1.2, 2026-11-01 | Rust | 6 | `draft` always; `send` only with `--allow-send` (or `MAIL_ALLOW_SEND`); no move or delete | Recipient allow-list (address or `@domain`; with no pattern nobody) and `--from-allow` | Per request in headers (HTTP) or environment (stdio); password, app password or OAuth access token (XOAUTH2); never stored |
| [mcp-email-server](https://github.com/Wh1isper/mcp-email-server) | 374 | 1.13.3, 2026-10-25 | Python | 21 | `allowed_mutations`: draft, organize, delete, send, append; **default: all five**; an empty list is read-only | Recipient and sender allow-lists; annotations on every tool | SQLite-backed managed configuration (legacy keyring); no OAuth found (?) |
| [google_workspace_mcp](https://github.com/taylorwilsdon/google_workspace_mcp) | 3.46k | v2.3.0, 2026-10-30 | Python | 120+ (about 15 for Gmail), not re-counted | `--read-only`, tool tiers, per-service permissions | Separate send gating unclear (?) | OAuth 2.1, multi-user |
| [ms-365-mcp-server](https://github.com/Softeria/ms-365-mcp-server) | 1.10k | v0.178.0, 2026-10-30 | TypeScript | 300+, not re-counted | `--read-only` | – | Device code or OAuth, encrypted token cache |
| [GongRzhe/Gmail-MCP-Server](https://github.com/GongRzhe/Gmail-MCP-Server) | 1.17k | none; **archived**, last commit 2025-08-06 | JavaScript | about 17 (?) | none | none | Local token file |

## emperor-mail: pros

- **Draft by default.** `draft` saves to the Drafts folder for a person to review. `send` refuses
  unless the server was started with `--allow-send`, and even then every To and Cc recipient must
  match a `--send-allow` pattern; with none, every recipient is refused. That is stricter than
  mcp-email-server's default, which allows all five mutation classes, send and delete included.
- **The sender is limited too.** `--from-allow` stops a caller from writing as someone else.
- **Credentials are never stored.** Every other server here keeps them in a file, a database, a
  keyring or a token cache. emperor-mail uses them for one call and drops them, OAuth tokens
  included: the client obtains and refreshes the token, the server only forwards it.
- **Sign-in failures the client can act on.** A rejected login or expired token gives HTTP 401 on
  the MCP response, the cue to sign in again or refresh the token.
- **Sent mail is filed.** The exact bytes sent are copied to the Sent folder, so the person sees
  what the assistant sent in their own mail client (`--no-sent-copy` for providers that do it
  themselves).
- **Markdown in, text and HTML out**, with raw HTML escaped and images turned into links: no
  tracking pixels, and no data hidden in image URLs for the recipient's client to fetch.
- **Attachments as text**: PDF, Word (DOCX), Excel (XLSX, XLSM, XLSB, XLS) and ODS, pure Rust.
- **Reading changes nothing**: folders opened read-only, messages fetched with `BODY.PEEK`.
- **Shared server**: one HTTP server can serve many users and providers; IMAP and SMTP hosts may
  come from headers (TLS only, and only with the caller's own login), or be pinned.
- **6 tools** instead of 21 to 300, annotated, tested live against GreenMail in CI.
- **One Rust binary with provenance** instead of an npm or PyPI package: a smaller supply-chain
  target after postmark-mcp.

## emperor-mail: cons

- **No organising**: no move, archive, flag or delete. mcp-email-server has all of them.
- **No forward**; replies thread with `in_reply_to`, but attachments cannot be re-sent.
- **One mailbox per stdio process**; mcp-email-server handles many accounts in one.
- **No OAuth flow of its own.** The client must get and refresh the token. Right for a gateway,
  more work for a desktop user than google_workspace_mcp or ms-365-mcp-server, which run the flow.
- **IMAP and SMTP only**: no Gmail labels or Microsoft Graph features (categories, focused inbox,
  calendar).
- `send` stays listed even when sending is off (deliberate: its refusal points the model to
  `draft`), so the tool list does not shrink like the other emperor servers' do.
- New, with no users yet.

## Where a competitor is better

- **mcp-email-server**: organise and delete behind their own switches, forward, several accounts,
  tags, a web UI for configuration. If you want a full mail assistant and
  will configure its mutation classes carefully, it does more.
- **google_workspace_mcp** and **ms-365-mcp-server**: the whole suite (calendar, drive, contacts)
  with the provider's own API and a server-run OAuth flow.

## When to pick something else

- You want the assistant to clean up the inbox (archive, move, delete): mcp-email-server.
- You live in Google Workspace or Microsoft 365 and want calendar and files too: the
  provider-specific servers.
- You want one desktop process for several mailboxes: mcp-email-server.

Pick emperor-mail when the assistant reads mail from strangers and must not be able to send
anything you did not allow, when one server serves several people without holding their
passwords, or when you want a single audited binary rather than a package installed at start-up.
