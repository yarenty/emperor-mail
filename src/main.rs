//! `emperor-mail` command line: `serve` and `doctor`.

use std::net::SocketAddr;

use clap::builder::FalseyValueParser;
use clap::{ArgAction, Args, Parser, Subcommand};
use emperor_mail::config::Settings;
use emperor_mail::doctor::{self, Plan};
use emperor_mail::policy::FromPolicy;
use emperor_mail::server::Options;
use emperor_mail::{MailServer, SendPolicy, forward_config, init_tracing};
use emperor_mcp::{ServeOptions, serve};
use rmcp::ServiceExt;

#[derive(Parser)]
#[command(
    name = "emperor-mail",
    version,
    about = "MCP server for email over IMAP and SMTP"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server (Streamable HTTP at /mcp, or stdio with --stdio).
    Serve(RunArgs),
    /// Check configuration, credentials and the mail servers, then exit (1 on failure).
    Doctor(RunArgs),
}

#[derive(Args, Clone)]
struct RunArgs {
    /// HTTP listen address.
    #[arg(long, default_value = "127.0.0.1:8033")]
    bind: SocketAddr,
    /// Allow the `send` tool (otherwise only `draft`). From the environment: unset, empty,
    /// `false`, `0` or `no` mean off.
    #[arg(long, env = "MAIL_ALLOW_SEND", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new())]
    allow_send: bool,
    /// Recipients `send` may write to: an address (ann@example.org) or a domain (@example.org).
    /// Repeat or separate with commas.
    #[arg(
        long = "send-allow",
        env = "MAIL_SEND_ALLOW",
        value_name = "PATTERN",
        value_delimiter = ','
    )]
    send_allow: Vec<String>,
    /// From addresses `send` and `draft` may use: an address or a domain (@example.org). Repeat or
    /// separate with commas. Unset: any.
    #[arg(
        long = "from-allow",
        env = "MAIL_FROM_ALLOW",
        value_name = "PATTERN",
        value_delimiter = ','
    )]
    from_allow: Vec<String>,
    /// Do not copy sent messages to the sent folder (for providers that save them already, such
    /// as Gmail).
    #[arg(long)]
    no_sent_copy: bool,
    /// Always use the IMAP/SMTP servers from the environment; ignore X-Mail-Imap-* and
    /// X-Mail-Smtp-* headers.
    #[arg(long)]
    pin_servers: bool,
    /// Speak MCP over stdin/stdout for one local mailbox (credentials from the environment).
    #[arg(long)]
    stdio: bool,
}

/// Patterns from a flag or comma-separated variable, without empty items (`MAIL_SEND_ALLOW=""`
/// or a trailing comma mean no pattern, not an invalid one).
fn patterns(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}

impl RunArgs {
    fn policy(&self) -> anyhow::Result<SendPolicy> {
        let send_allow = patterns(&self.send_allow);
        if !self.allow_send && !send_allow.is_empty() {
            anyhow::bail!(
                "--send-allow (MAIL_SEND_ALLOW) has no effect without --allow-send (MAIL_ALLOW_SEND)"
            );
        }
        Ok(SendPolicy::new(self.allow_send, &send_allow)?)
    }

    fn options(&self) -> anyhow::Result<Options> {
        Ok(Options {
            from_policy: FromPolicy::new(&patterns(&self.from_allow))?,
            sent_copy: !self.no_sent_copy,
            pin_servers: self.pin_servers,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) => run_serve(args).await,
        Command::Doctor(args) => {
            let options = args.options()?;
            let plan = Plan {
                bind: (!args.stdio).then_some(args.bind),
                policy: args.policy()?,
                from_policy: options.from_policy,
                sent_copy: options.sent_copy,
                pin_servers: options.pin_servers,
            };
            let report = doctor::run(&Settings::from_env(), &plan).await;
            report.print();
            std::process::exit(report.exit_code());
        }
    }
}

async fn run_serve(args: RunArgs) -> anyhow::Result<()> {
    let policy = args.policy()?;
    let options = args.options()?;
    let env = Settings::from_env();
    tracing::info!(
        policy = %policy.summary(),
        from = %options.from_policy.summary(),
        sent_copy = options.sent_copy,
        pin_servers = options.pin_servers,
        "emperor-mail {}",
        env!("CARGO_PKG_VERSION")
    );
    let server = MailServer::with_options(env.clone(), policy, options);
    if args.stdio {
        if !env.has_login() {
            tracing::warn!(
                "MAIL_USER with MAIL_PASSWORD or MAIL_OAUTH_TOKEN not set; every tool call will fail"
            );
        }
        let running = server.serve(rmcp::transport::stdio()).await?;
        running.waiting().await?;
        return Ok(());
    }
    if env.has_login() && !args.bind.ip().is_loopback() {
        tracing::warn!(
            bind = %args.bind,
            "MAIL_USER is set and the server listens beyond loopback: every caller without \
             X-Mail-* headers reads this mailbox. Put it behind an authenticating ingress."
        );
    }
    let options = ServeOptions::new(args.bind).with_forward(forward_config());
    serve(options, move || server.clone()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const VARS: [&str; 3] = ["MAIL_ALLOW_SEND", "MAIL_SEND_ALLOW", "MAIL_FROM_ALLOW"];

    fn parse(env: &[(&str, &str)], args: &[&str]) -> RunArgs {
        // SAFETY: this is the only test that touches these variables, and it runs every case in
        // sequence on one thread.
        unsafe {
            for v in VARS {
                std::env::remove_var(v);
            }
            for (k, v) in env {
                std::env::set_var(k, v);
            }
        }
        let cli = Cli::try_parse_from(["emperor-mail", "serve"].iter().chain(args))
            .unwrap_or_else(|e| panic!("{env:?} {args:?}: {e}"));
        match cli.command {
            Command::Serve(a) => a,
            Command::Doctor(_) => unreachable!(),
        }
    }

    #[test]
    fn send_switches_from_the_environment() {
        // Off unless switched on; falsey values are off.
        assert!(!parse(&[], &[]).allow_send);
        for off in ["", "false", "0", "no", "False", "off"] {
            let a = parse(&[("MAIL_ALLOW_SEND", off)], &[]);
            assert!(!a.allow_send, "MAIL_ALLOW_SEND={off:?} must be off");
            assert!(!a.policy().unwrap().enabled);
        }
        for on in ["true", "1", "yes"] {
            assert!(
                parse(&[("MAIL_ALLOW_SEND", on)], &[]).allow_send,
                "MAIL_ALLOW_SEND={on:?} must be on"
            );
        }
        assert!(parse(&[], &["--allow-send"]).allow_send);

        // Comma-separated patterns; empty means none.
        let a = parse(
            &[
                ("MAIL_ALLOW_SEND", "true"),
                ("MAIL_SEND_ALLOW", "@example.org, boss@partner.example"),
                ("MAIL_FROM_ALLOW", "@corp.example"),
            ],
            &[],
        );
        let policy = a.policy().unwrap();
        assert_eq!(policy.patterns.len(), 2);
        assert!(
            policy
                .check(["ann@example.org", "boss@partner.example"])
                .is_ok()
        );
        assert!(policy.check(["eve@elsewhere.example"]).is_err());
        assert_eq!(a.options().unwrap().from_policy.patterns.len(), 1);

        // Sending on with no pattern (unset or empty): every recipient is refused.
        for env in [
            vec![("MAIL_ALLOW_SEND", "1")],
            vec![("MAIL_ALLOW_SEND", "1"), ("MAIL_SEND_ALLOW", "")],
            vec![("MAIL_ALLOW_SEND", "1"), ("MAIL_SEND_ALLOW", " , ")],
        ] {
            let a = parse(&env, &[]);
            let policy = a.policy().unwrap();
            assert!(policy.enabled && policy.patterns.is_empty(), "{env:?}");
            assert!(policy.check(["ann@example.org"]).is_err(), "{env:?}");
            assert!(a.options().unwrap().from_policy.patterns.is_empty());
        }

        // Patterns without sending on are a mistake, not a silent no-op.
        let a = parse(&[("MAIL_SEND_ALLOW", "@example.org")], &[]);
        assert!(a.policy().is_err());
        // A bad pattern from the environment is refused like one on the command line.
        let a = parse(
            &[
                ("MAIL_ALLOW_SEND", "yes"),
                ("MAIL_SEND_ALLOW", "example.org"),
            ],
            &[],
        );
        assert!(a.policy().is_err());

        // The command line wins over the environment.
        let a = parse(
            &[("MAIL_SEND_ALLOW", "@env.example")],
            &["--allow-send", "--send-allow", "@flag.example"],
        );
        assert_eq!(a.policy().unwrap().patterns.len(), 1);
        assert!(a.policy().unwrap().check(["x@flag.example"]).is_ok());

        unsafe {
            for v in VARS {
                std::env::remove_var(v);
            }
        }
    }
}
