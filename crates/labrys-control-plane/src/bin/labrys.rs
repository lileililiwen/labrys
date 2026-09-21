//! The `labrys` command-line interface.
//!
//! Every subcommand maps to one stable lifecycle command on the control-plane
//! API and prints one machine-readable JSON envelope to stdout. Failures also
//! print their actionable recovery to stderr. Agent actors cannot invoke
//! human-only lifecycle mutations; secrets are rendered as references or
//! redaction markers, never values.
//!
//! Exit codes: `0` success, `1` the command ran but failed (denied,
//! unprocessable, failed state), `2` usage, authentication, connection, or
//! protocol errors.

use std::collections::BTreeMap;

use clap::{Parser, Subcommand};

use labrys_control_plane::{exit_for, recovery_of, ControlPlaneClient, ExitCode};

#[derive(Debug, Parser)]
#[command(name = "labrys", version, about = "Labrys application platform CLI")]
struct Cli {
    /// Control-plane API base URL.
    #[arg(long, env = "LABRYS_API_URL", default_value = "http://127.0.0.1:8080")]
    api_url: String,
    /// Bearer token for the control-plane API.
    #[arg(long, env = "LABRYS_API_TOKEN", default_value = "")]
    token: String,
    /// Actor identity: human:<name>, agent:<session>:<name>, or platform:<name>.
    #[arg(long, env = "LABRYS_ACTOR", default_value = "human:operator")]
    actor: String,
    /// Trace id for request correlation.
    #[arg(long, env = "LABRYS_TRACE_ID")]
    trace_id: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show API and protocol versions.
    Version,
    /// Import a project from a local path.
    #[command(alias = "init")]
    Import {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = ".")]
        path: String,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Inspect a registered application.
    Inspect {
        #[arg(long)]
        application: String,
    },
    /// Start the development runtime.
    #[command(alias = "dev")]
    Run {
        #[arg(long)]
        application: String,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Request agent work through a session.
    Agent {
        #[arg(long)]
        application: String,
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Open an isolated preview.
    Preview {
        #[arg(long)]
        application: String,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Provision a capability (secret values stay references only).
    Capability {
        #[arg(long)]
        application: String,
        #[arg(long)]
        capability: String,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// List bound capabilities.
    Capabilities {
        #[arg(long)]
        application: String,
    },
    /// List platform resources.
    Resources {
        #[arg(long)]
        application: String,
    },
    /// Build an OCI artifact (promotion waits for health).
    Build {
        #[arg(long)]
        application: String,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Deploy to an environment (humans only for production).
    Deploy {
        #[arg(long)]
        application: String,
        #[arg(long, default_value = "production")]
        environment: String,
        #[arg(long)]
        approve_by: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// List deployments with pagination.
    Deployments {
        #[arg(long)]
        application: String,
        #[arg(long)]
        environment: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Show redacted platform logs.
    Logs {
        #[arg(long)]
        application: String,
        #[arg(long)]
        source: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Show platform health (never healthy without observed evidence).
    Health {
        #[arg(long)]
        application: String,
        #[arg(long)]
        environment: Option<String>,
    },
    /// Roll back to a prior deployment (named human approval required).
    Rollback {
        #[arg(long)]
        application: String,
        #[arg(long)]
        target: String,
        #[arg(long)]
        approve_by: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Diagnose an application with recovery guidance.
    Doctor {
        #[arg(long)]
        application: String,
    },
    /// Show durable job status.
    Job {
        #[arg(long)]
        job: String,
    },
    /// Negotiate the agent protocol version.
    Negotiate {
        #[arg(long, default_value_t = 1)]
        protocol_version: u32,
        #[arg(long)]
        backend: Option<String>,
    },
    /// List registered plugins.
    Plugins,
}

fn key_or_generated(explicit: Option<String>, command: &str) -> String {
    explicit.unwrap_or_else(|| format!("{command}-{}", uuid::Uuid::new_v4().simple()))
}

fn emit(envelope: &serde_json::Value) -> ExitCode {
    println!(
        "{}",
        serde_json::to_string_pretty(envelope).unwrap_or_else(|_| "{}".to_string())
    );
    let code = exit_for(envelope);
    if code != ExitCode::Ok {
        if let Some(recovery) = recovery_of(envelope) {
            eprintln!("recovery: {recovery}");
        }
    }
    code
}

fn fail_usage(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(ExitCode::Usage as i32);
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let client = match &cli.trace_id {
        Some(trace) => ControlPlaneClient::with_trace(
            cli.api_url.clone(),
            cli.token.clone(),
            cli.actor.clone(),
            trace.clone(),
        ),
        None => ControlPlaneClient::new(cli.api_url.clone(), cli.token.clone(), cli.actor.clone()),
    };
    let mut client = match client {
        Ok(client) => client,
        Err(message) => fail_usage(&message),
    };
    let _ = &mut client;
    let result = run(cli.command, &client).await;
    let code = match result {
        Ok(envelope) => emit(&envelope),
        Err(message) => {
            eprintln!("error: {message}");
            if message.contains("recovery:") {
                // Server-provided guidance is already in the message.
            }
            ExitCode::Usage
        }
    };
    std::process::exit(code as i32);
}

async fn run(command: Command, client: &ControlPlaneClient) -> Result<serde_json::Value, String> {
    match command {
        Command::Version => client.version().await,
        Command::Import {
            name,
            path,
            idempotency_key,
        } => {
            client
                .import(
                    name.as_deref(),
                    Some(&path),
                    &key_or_generated(idempotency_key, "import"),
                )
                .await
        }
        Command::Inspect { application } => client.inspect(&application).await,
        Command::Run {
            application,
            idempotency_key,
        } => {
            client
                .run(
                    &application,
                    &key_or_generated(idempotency_key, "run"),
                    &BTreeMap::new(),
                )
                .await
        }
        Command::Agent {
            application,
            prompt,
            idempotency_key,
        } => {
            let mut args = BTreeMap::new();
            if let Some(prompt) = prompt {
                args.insert("prompt".to_string(), prompt);
            }
            client
                .agent(
                    &application,
                    &key_or_generated(idempotency_key, "agent"),
                    &args,
                )
                .await
        }
        Command::Preview {
            application,
            session_id,
            idempotency_key,
        } => {
            let mut args = BTreeMap::new();
            if let Some(session) = session_id {
                args.insert("session_id".to_string(), session);
            }
            client
                .preview(
                    &application,
                    &key_or_generated(idempotency_key, "preview"),
                    &args,
                )
                .await
        }
        Command::Capability {
            application,
            capability,
            idempotency_key,
        } => {
            client
                .capability(
                    &application,
                    &capability,
                    &key_or_generated(idempotency_key, "capability"),
                )
                .await
        }
        Command::Capabilities { application } => client.capabilities(&application).await,
        Command::Resources { application } => client.resources(&application).await,
        Command::Build {
            application,
            idempotency_key,
        } => {
            client
                .build(
                    &application,
                    &key_or_generated(idempotency_key, "build"),
                    &BTreeMap::new(),
                )
                .await
        }
        Command::Deploy {
            application,
            environment,
            approve_by,
            idempotency_key,
        } => {
            client
                .deploy(
                    &application,
                    &environment,
                    &key_or_generated(idempotency_key, "deploy"),
                    approve_by.as_deref(),
                )
                .await
        }
        Command::Deployments {
            application,
            environment,
            limit,
        } => {
            client
                .deployments(&application, environment.as_deref(), limit)
                .await
        }
        Command::Logs {
            application,
            source,
            limit,
        } => client.logs(&application, source.as_deref(), limit).await,
        Command::Health {
            application,
            environment,
        } => client.health(&application, environment.as_deref()).await,
        Command::Rollback {
            application,
            target,
            approve_by,
            idempotency_key,
        } => {
            let Some(approver) = approve_by else {
                return Err("rollback requires --approve-by <name>: a granted, named human approval (no rollback was enqueued)".to_string());
            };
            client
                .rollback(
                    &application,
                    &target,
                    &approver,
                    &key_or_generated(idempotency_key, "rollback"),
                )
                .await
        }
        Command::Doctor { application } => client.doctor(&application).await,
        Command::Job { job } => client.job(&job).await,
        Command::Negotiate {
            protocol_version,
            backend,
        } => {
            client
                .negotiate_agent(protocol_version, backend.as_deref())
                .await
        }
        Command::Plugins => client.plugins().await,
    }
}
