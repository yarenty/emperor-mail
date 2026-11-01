# emperor-mail: MCP server for email over IMAP and SMTP.
# Runs one mailbox over stdio; credentials come from the environment (MAIL_USER,
# MAIL_PASSWORD or MAIL_OAUTH_TOKEN, IMAP_HOST, SMTP_HOST, ...). See README.md.

FROM rust:1.88-slim-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked \
    && strip target/release/emperor-mail

FROM debian:bookworm-slim
ARG VERSION=0.1.1
LABEL org.opencontainers.image.title="emperor-mail" \
      org.opencontainers.image.description="MCP server for email: search, read, draft and (guarded) send over IMAP and SMTP" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.source="https://github.com/yarenty/emperor-mail" \
      org.opencontainers.image.url="https://github.com/yarenty/emperor-mail" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0" \
      org.opencontainers.image.authors="yarenty <yarenty@gmail.com>"
# rustls verifies certificates against the operating system's trust store.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin emperor
COPY --from=builder /src/target/release/emperor-mail /usr/local/bin/emperor-mail
USER 10001
ENTRYPOINT ["emperor-mail", "serve", "--stdio"]
