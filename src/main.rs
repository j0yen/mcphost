use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use mcphost::db::Db;
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::HttpKind;
use mcphost::kinds::python::PythonKind;
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
    Serve {
        /// Enable the P1 registry-publish feature (`host.registry_publish`,
        /// `GET /.well-known/mcp/<namespace>/server.json`) and name the
        /// registry API's base URL, e.g.
        /// `https://registry.modelcontextprotocol.io`. Off by default (PRD
        /// requirement 15's feature flag). Also settable via
        /// `$MCPHOST_REGISTRY_URL`; this flag takes precedence.
        #[arg(long)]
        registry_url: Option<String>,
    },
    /// Apply pending database migrations and exit.
    Migrate {
        /// PRD-mcphost-migration-safety requirement 2: instead of applying
        /// migrations to the live database, copy it, apply pending
        /// migrations to the copy, then prove `--previous`'s binary can
        /// still run against the migrated copy (initialize, tools/list, a
        /// control-plane tools/call, and /healthz). Exits 0 on success, 4
        /// naming the failing step otherwise. The live database is never
        /// touched.
        #[arg(long)]
        check_compat: bool,
        /// Path to the previous release's `mcphost` binary. Required with
        /// `--check-compat`.
        #[arg(long)]
        previous: Option<PathBuf>,
        /// The sqlite file to check against `--check-compat`. Defaults to
        /// `$MCPHOST_DATA_DIR/mcphost.db` (the normal live database path).
        #[arg(long)]
        db: Option<PathBuf>,
    },
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
        Command::Migrate {
            check_compat,
            previous,
            db: db_path,
        } => {
            init_tracing();
            if check_compat {
                let Some(previous_bin) = previous else {
                    eprintln!("mcphost migrate --check-compat requires --previous <path>");
                    std::process::exit(2);
                };
                let live_db = db_path.unwrap_or_else(|| data_dir().join("mcphost.db"));
                match mcphost::compat_check::run(&live_db, &previous_bin).await {
                    Ok(()) => {
                        println!("check-compat: ok (previous release runs on the migrated schema)");
                        Ok(())
                    }
                    Err(failure) => {
                        eprintln!("check-compat: FAILED at step '{}': {}", failure.step, failure.detail);
                        std::process::exit(4);
                    }
                }
            } else {
                let db = Db::open(&data_dir())?;
                db.migrate().await?;
                tracing::info!("migrations applied");
                Ok(())
            }
        }
        Command::Serve { registry_url } => {
            init_tracing();

            let bind: std::net::SocketAddr = env_or("MCPHOST_BIND", "127.0.0.1:8080").parse()?;
            let public_url = env_or("MCPHOST_PUBLIC_URL", &format!("http://{bind}"));
            let admin_key = std::env::var("MCPHOST_ADMIN_KEY").ok();
            let secret_key = env_or("MCPHOST_SECRET_KEY", "mcphost-dev-secret-key-change-me");

            if admin_key.is_none() {
                tracing::warn!("MCPHOST_ADMIN_KEY is unset; admin.* tools are unreachable");
            }

            // AC19 / requirement 15: off by default. `--registry-url` wins
            // over `$MCPHOST_REGISTRY_URL` when both are given.
            let registry_url = registry_url.or_else(|| std::env::var("MCPHOST_REGISTRY_URL").ok());
            let registry =
                registry_url.map(|base_url| mcphost::registry::RegistryConfig { base_url });
            if registry.is_none() {
                tracing::info!(
                    "registry-publish is disabled (no --registry-url / $MCPHOST_REGISTRY_URL)"
                );
            }

            let db = Db::open(&data_dir())?;
            db.migrate().await?;

            // Requirement 3's "the host's own domain" SSRF rule: a
            // published `http` tool may never target this host's own
            // public endpoint.
            let mut kinds = KindRegistry::with_builtin();
            let own_domain = mcphost::kinds::http::own_domain_from_url(&public_url);
            kinds.register(std::sync::Arc::new(HttpKind::new(own_domain)?));
            let python_kind = PythonKind::new(&data_dir());
            let sandbox_mechanism = Some(python_kind.mechanism());
            tracing::info!(
                mechanism = sandbox_mechanism,
                "python kind sandbox mechanism"
            );
            kinds.register(std::sync::Arc::new(python_kind));

            let state = Arc::new(AppState {
                db,
                kinds,
                secrets: SecretBox::from_passphrase(&secret_key),
                admin_key,
                public_url,
                call_timeout: mcphost::state::CALL_TIMEOUT,
                registry,
                http_client: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()?,
                sandbox_mechanism,
            });

            mcphost::http::serve(bind, state).await
        }
    }
}
