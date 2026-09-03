use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use mcphost::db::Db;
use mcphost::kinds::KindRegistry;
use mcphost::secrets::SecretBox;
use mcphost::state::AppState;

#[derive(Parser)]
#[command(
    name = "mcphost",
    version,
    about = "One MCP endpoint an agent can join and administer without a human"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the streamable-HTTP MCP server.
    Serve,
    /// Apply pending database migrations and exit.
    Migrate,
    /// Print the version and exit.
    Version,
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn data_dir() -> PathBuf {
    PathBuf::from(env_or("MCPHOST_DATA_DIR", "./data"))
}

fn init_tracing() {
    let level = env_or("MCPHOST_LOG_LEVEL", "info");
    let filter = tracing_subscriber::EnvFilter::try_new(&level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Version => {
            println!("mcphost {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Migrate => {
            init_tracing();
            let db = Db::open(&data_dir())?;
            db.migrate().await?;
            tracing::info!("migrations applied");
            Ok(())
        }
        Command::Serve => {
            init_tracing();

            let bind: std::net::SocketAddr = env_or("MCPHOST_BIND", "127.0.0.1:8080").parse()?;
            let public_url = env_or("MCPHOST_PUBLIC_URL", &format!("http://{bind}"));
            let admin_key = std::env::var("MCPHOST_ADMIN_KEY").ok();
            let secret_key = env_or("MCPHOST_SECRET_KEY", "mcphost-dev-secret-key-change-me");

            if admin_key.is_none() {
                tracing::warn!("MCPHOST_ADMIN_KEY is unset; admin.* tools are unreachable");
            }

            let db = Db::open(&data_dir())?;
            db.migrate().await?;

            let state = Arc::new(AppState {
                db,
                kinds: KindRegistry::with_builtin(),
                secrets: SecretBox::from_passphrase(&secret_key),
                admin_key,
                public_url,
                call_timeout: mcphost::state::CALL_TIMEOUT,
            });

            mcphost::http::serve(bind, state).await
        }
    }
}
