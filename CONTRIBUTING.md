# Contributing to emperor-mail

Thanks for helping. Small, focused pull requests are the easiest to review; open an issue first
for anything that changes the credential rules, the send guard or the tool list (see
[AGENTS.md](./AGENTS.md)).

## Before you open a pull request

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check
```

If you touch IMAP, SMTP or drafts, also run the live test against GreenMail (needs Docker):

```bash
docker pull greenmail/standalone:2.1.5
cargo test --test greenmail -- --ignored
```

Keep the docs in step with the code: `README.md`, `AGENTS.md`, `manifest.yaml`, and a
`CHANGELOG.md` entry under `[Unreleased]` for anything a user would notice. Every public item
is documented (`#![warn(missing_docs)]`).

Never put real credentials in code, tests, fixtures or issues. Tests that need a mailbox make one
up for the run (see `tests/greenmail.rs`).

## Developer Certificate of Origin

This project uses the [Developer Certificate of Origin](https://developercertificate.org/)
(DCO) instead of a contributor licence agreement. By adding a sign-off line to your commits
you certify that you wrote the change or otherwise have the right to submit it under the
project licence.

Sign off every commit:

```bash
git commit -s
```

which adds a line such as

```
Signed-off-by: Your Name <you@example.com>
```

Use a real name and a reachable email address.

## Licence

emperor-mail is dual licensed under MIT OR Apache-2.0 (see [LICENSE-MIT](./LICENSE-MIT) and
[LICENSE-APACHE](./LICENSE-APACHE)). Unless you explicitly state otherwise, any contribution
you submit is licensed the same way, without any additional terms or conditions.
