//! `emperor-mail` command line: `serve` and `doctor`.

use std::net::SocketAddr;

use clap::{Args, Parser, Subcommand};
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
    /// Allow the `send` tool (otherwise only `draft`).
    #[arg(long)]
    allow_send: bool,
    /// Recipients `send` may write to: an address (ann@example.org) or a domain (@example.org).
    /// Repeat or separate with commas.
    #[arg(long = "send-allow", value_name = "PATTERN", value_delimiter = ',')]
    send_allow: Vec<String>,
    /// From addresses `send` and `draft` may use: an address or a domain (@example.org). Repeat or
    /// separate with commas. Unset: any.
    #[arg(long = "from-allow", value_name = "PATTERN", value_delimiter = ',')]
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

impl RunArgs {
    fn policy(&self) -> anyhow::Result<SendPolicy> {
        if !self.allow_send && !self.send_allow.is_empty() {
            anyhow::bail!("--send-allow has no effect without --allow-send");
        }
        Ok(SendPolicy::new(self.allow_send, &self.send_allow)?)
    }

    fn options(&self) -> anyhow::Result<Options> {
        Ok(Options {
            from_policy: FromPolicy::new(&self.from_allow)?,
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
