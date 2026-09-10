use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use mcphost::db::Db;
use mcphost::kinds::KindRegistry;
use mcphost::kinds::chain::ChainKind;
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
    /// PRD-mcphost-sandbox-ready P2 requirement 9 / AC10: run the sandbox
    /// self-test in-process and exit 0 (ready) or 1 (unready), printing the
    /// same detail line `/healthz` would show -- for a shell on the host,
    /// without hitting the HTTP surface.
    SandboxCheck,
    /// PRD-mcphost-metered-overage: offline billing-related subcommands.
    Billing {
        #[command(subcommand)]
        action: BillingCommand,
    },
    /// PRD-mcphost-tenant-attribution P0 requirement 4 (AC5): signups ->
    /// published -> first successful call -> returned -> hit the daily
    /// cap -> upgraded, split real (`source_class = external`) from
    /// synthetic -- an admin CLI report on the box, same binary, reading
    /// only the `tenants`/`tools`/`calls` tables. Meant to run ad hoc or
    /// from the measure job after each run (technical considerations).
    Funnel {
        /// Only tenants that signed up on or after this UTC date
        /// (`YYYY-MM-DD`). Unset reports on every tenant.
        #[arg(long)]
        since: Option<String>,
        /// Print the report as JSON instead of the default text table.
        #[arg(long)]
        json: bool,
    },
    /// PRD-mcphost-surface-fluidity requirement 4 (AC5): regenerate
    /// `www/llms.txt`'s generated `## Tools` section from the same
    /// descriptors `tools/list` serves, so the two can never drift the way
    /// the 2026-09-09 audit found (14 documented vs 18 live). Needs no DB
    /// and no sandbox: the tool name set is independent of which optional
    /// kinds (http/python) are registered, so this always builds against
    /// `KindRegistry::with_builtin()` (echo only).
    LlmsTxt {
        /// Exit 1 without writing if the file is stale, instead of
        /// rewriting it -- same convention as `scripts/gen-llms-full.sh
        /// --check`, for a pre-commit/CI gate.
        #[arg(long)]
        check: bool,
        /// Path to the llms.txt file to update. Defaults to `www/llms.txt`
        /// relative to the current directory (run from the repo root).
        #[arg(long)]
        path: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum BillingCommand {
    /// Requirement 51: read pro tenants' unemitted `ok` calls, POST one
    /// Stripe meter event per tenant (chunked at 100 events/request), and
    /// advance the high-water mark only once every event in this run has
    /// been ledgered. Meant to run from `deploy/mcphost-emit-meter.timer`
    /// every five minutes; exits non-zero (leaving state untouched) on any
    /// POST failure.
    EmitMeter,
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
        Command::SandboxCheck => {
            // Deliberately no `init_tracing()`: this subcommand's contract
            // (AC10) is "prints the same detail line /healthz would show"
            // on stdout -- a JSON log line ahead of it (tracing's default
            // writer is stdout) would break a shell script's `$(mcphost
            // sandbox-check)` capture. Without a global subscriber
            // installed, `tracing::*!` calls elsewhere in this path are
            // simply no-ops, not an error.
            let python_kind = PythonKind::new(&data_dir());
            let status = python_kind.run_startup_selftest().await;
            println!("{}", status.detail);
            if status.ready {
                Ok(())
            } else {
                std::process::exit(1);
            }
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
                        eprintln!(
                            "check-compat: FAILED at step '{}': {}",
                            failure.step, failure.detail
                        );
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
        Command::Billing { action } => match action {
            BillingCommand::EmitMeter => {
                init_tracing();
                let dir = data_dir();
                // Requirement 51: a timer overlap must not double-read the
                // same span -- held for the whole subcommand, dropped (and
                // so released) when this match arm returns.
                let _lock = match mcphost::metering::acquire_lock(&dir) {
                    Ok(lock) => lock,
                    Err(e) => {
                        eprintln!("emit-meter: {e}");
                        std::process::exit(3);
                    }
                };
                let db = Db::open(&dir)?;
                db.migrate().await?;
                let billing_config = mcphost::billing::BillingConfig::from_env();
                let Some(secret_key) = billing_config.secret_key.clone() else {
                    eprintln!("emit-meter: MCPHOST_STRIPE_SECRET_KEY is not set");
                    std::process::exit(2);
                };
                let http_client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()?;
                let client = mcphost::billing::StripeClient::new(http_client, secret_key);
                let event_name = billing_config.meter_event_name().to_string();
                match mcphost::metering::run_once(&db, &client, &event_name).await {
                    Ok(outcome) => {
                        println!(
                            "emit-meter: batch {} sent {} event(s) across {} request(s) covering {} call(s)",
                            outcome.batch_id,
                            outcome.events_sent,
                            outcome.requests_sent,
                            outcome.calls_covered
                        );
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("emit-meter: {e}");
                        std::process::exit(1);
                    }
                }
            }
        },
        Command::Funnel { since, json } => {
            // Deliberately no `init_tracing()`: same rationale as
            // `SandboxCheck` above -- this subcommand's whole contract is
            // stdout output a shell (or the measure job) parses, and a
            // JSON log line ahead of it would break `--json` output the
            // same way it would break `$(mcphost sandbox-check)`.
            let since_unix = match since.as_deref() {
                Some(s) => match mcphost::state::parse_date_ymd_unix(s) {
                    Some(u) => Some(u),
                    None => {
                        eprintln!("funnel: --since '{s}' is not a valid YYYY-MM-DD date");
                        std::process::exit(2);
                    }
                },
                None => None,
            };
            let dir = data_dir();
            let db = Db::open(&dir)?;
            db.migrate().await?;
            let plans_path = mcphost::plans::PlanCatalog::path_from_env(&dir);
            let plans = mcphost::plans::PlanCatalog::load_or_init(&plans_path)?;
            let report = mcphost::funnel::compute(&db, &plans, since_unix).await?;
            if json {
                println!("{}", serde_json::to_string(&report.to_json())?);
            } else {
                for (label, stage) in [("real", &report.real), ("synthetic", &report.synthetic)] {
                    println!(
                        "{label}: signed_up={} published={} called={} returned={} hit_cap={} upgraded={} median_signup_to_first_call_secs={}",
                        stage.signed_up,
                        stage.published,
                        stage.called,
                        stage.returned,
                        stage.hit_cap,
                        stage.upgraded,
                        stage
                            .median_signup_to_first_call_secs
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "n/a".to_string()),
                    );
                }
            }
            Ok(())
        }
        Command::LlmsTxt { check, path } => {
            // Deliberately no `init_tracing()`: same rationale as
            // `SandboxCheck`/`Funnel` above -- this subcommand's contract is
            // plain stdout/exit-code, no JSON log line ahead of it.
            let path = path.unwrap_or_else(|| PathBuf::from("www/llms.txt"));
            let existing = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                eprintln!("llms-txt: failed to read {}: {e}", path.display());
                std::process::exit(2);
            });
            let kinds = KindRegistry::with_builtin();
            let section = mcphost::llms_txt::render_tools_section(&kinds);
            let updated = mcphost::llms_txt::splice_into(&existing, &section);
            if check {
                if updated == existing {
                    println!("llms-txt --check: {} is up to date", path.display());
                    Ok(())
                } else {
                    eprintln!(
                        "llms-txt --check: {} is stale (run `mcphost llms-txt`)",
                        path.display()
                    );
                    std::process::exit(1);
                }
            } else {
                if updated != existing {
                    std::fs::write(&path, &updated)?;
                    println!("llms-txt: regenerated {}", path.display());
                } else {
                    println!("llms-txt: {} already up to date", path.display());
                }
                Ok(())
            }
        }
        Command::Serve { registry_url } => {
            init_tracing();

            // PRD-mcphost-call-limits-honest five-whys: RLIMIT_NPROC (the
            // fork-storm cap, requirement 6) is a kernel-level no-op for
            // root regardless of any prlimit/setrlimit call -- see
            // `sandbox::fork_storm_cap_is_reliable`'s doc comment for the
            // empirical trace. Every supported deployment runs mcphost as
            // an unprivileged systemd **user** unit, so a real uid of 0
            // here is a misconfiguration; fail loudly rather than silently
            // serve fork-storm-uncapped traffic.
            // SAFETY: getuid() takes no arguments and cannot fail.
            let real_uid = unsafe { libc::getuid() };
            mcphost::sandbox::refuse_to_serve_as_root(real_uid);

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

            let signup_rate_limit_per_hour = mcphost::state::signup_rate_limit_per_hour_from_env();

            // PRD-grand-loop-billing requirement 1 (AC1): loaded once at
            // startup, writing plans.toml with defaults if this data dir
            // has never seen one.
            let plans_path = mcphost::plans::PlanCatalog::path_from_env(&data_dir());
            let plans = mcphost::plans::PlanCatalog::load_or_init(&plans_path)?;
            let billing_config = mcphost::billing::BillingConfig::from_env();
            if billing_config.secret_key.is_none() {
                tracing::info!(
                    "billing is disabled (no MCPHOST_STRIPE_SECRET_KEY); quotas still enforce"
                );
            }
            let billing_http_client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?;
            // No key configured: `billing.checkout` always refuses with
            // `billing_unavailable` before this client would ever be
            // called (AC4), but `AppState` still needs a concrete value
            // for the field, so an empty secret key is a harmless stand-in.
            let stripe_secret_key = billing_config.secret_key.clone().unwrap_or_default();
            let billing_client: Arc<dyn mcphost::billing::BillingClient> = Arc::new(
                mcphost::billing::StripeClient::new(billing_http_client, stripe_secret_key),
            );

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
            // PRD-mcphost-sandbox-ready requirement 1: run the sandbox
            // self-test now, after `detect_mechanism`, and await it to
            // completion before the HTTP listener below ever starts
            // accepting connections -- so `/healthz`'s `sandbox_ready` is
            // never observably wrong (AC2: "checked_at is within 5s of
            // start") and never claims a mechanism it hasn't exercised
            // (goal 1). A failed self-test does not fail startup: `serve`
            // still runs below regardless (requirement 1).
            python_kind.run_startup_selftest().await;
            kinds.register(std::sync::Arc::new(python_kind));
            // PRD-mcphost-composition requirement 3: host-side only, no
            // sandbox/isolation concept of its own -- registered
            // unconditionally, unlike `http`/`python` above which need a
            // domain or a sandbox mechanism first.
            kinds.register(std::sync::Arc::new(ChainKind));

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
                tool_run_limiter: mcphost::state::ToolRunLimiter::new(),
                signup_rate_limit_per_hour,
                plans,
                billing_config,
                billing_client,
                checkout_sessions: std::sync::Arc::new(std::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
                accepted_usage_cache: std::sync::Arc::new(std::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
            });

            mcphost::http::serve(bind, state).await
        }
    }
}
