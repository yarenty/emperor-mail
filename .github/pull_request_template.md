## What changes

<!-- What a user or operator will notice. -->

## Checklist

- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` and `cargo deny check` pass
- [ ] IMAP, SMTP or draft changes: `cargo test --test greenmail -- --ignored` passes (needs Docker)
- [ ] `README.md`, `AGENTS.md`, `manifest.yaml` and `CHANGELOG.md` (`[Unreleased]`) updated where behaviour changed
- [ ] No real credentials, tokens or message content in code, tests or fixtures
- [ ] Every commit is signed off (`git commit -s`, [DCO](https://developercertificate.org/))
