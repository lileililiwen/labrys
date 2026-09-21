use std::sync::Arc;

use labrys_control_plane::{
    connect, run_migrations, select_dispatcher, ApiConfig, ApiState, Config, Worker,
};

/// Control-plane process entry point.
///
/// Loads database credentials and runtime settings from process configuration,
/// runs embedded migrations explicitly, probes the container runtime, and
/// starts the reconciliation workers.
///
/// Dispatcher selection comes from `LABRYS_RUNTIME_MODE` (auto/docker/disabled,
/// default auto): a reachable runtime runs the executable dispatcher (real
/// provider, container, and preview executors with job identity plumbed into
/// persisted events and logs); otherwise the daemon starts with the
/// no-op-blocked dispatcher and reports every execution stage as an
/// environment blocker with the recovery action, never a simulated pass.
/// `LABRYS_RUNTIME_MODE=docker` with an unreachable runtime fails startup
/// fast instead of starting blocked.
///
/// When `LABRYS_API_ADDR` is set (and `LABRYS_API_TOKEN` holds a valid
/// token), the process also serves the authenticated control-plane API on
/// that address; otherwise it runs workers only.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let pool = connect(&config).await?;
    if config.run_migrations {
        run_migrations(&pool).await?;
        println!("migrations applied");
    }

    let selected = select_dispatcher(&pool, &config).await?;
    match &selected.availability {
        labrys_control_plane::RuntimeAvailability::Available { docker_version } => {
            println!(
                "dispatcher=executable (mode {}, runtime {docker_version}, workspace {}, preview ttl {}s)",
                config.runtime_mode,
                config.workspace_root.display(),
                config.preview_ttl_secs,
            );
        }
        labrys_control_plane::RuntimeAvailability::Unavailable { reason } => {
            println!(
                "dispatcher=noop-blocked (mode {}): BLOCKED — {reason}; recovery: {}",
                config.runtime_mode,
                labrys_control_plane::BLOCKER_RECOVERY,
            );
        }
    }

    let dispatcher = selected.dispatcher;
    let mut workers = Vec::new();
    let mut handles = Vec::new();
    for index in 0..config.max_concurrency {
        let mut worker_config = config.clone();
        worker_config.worker_id = format!("{}-{index}", config.worker_id);
        let worker = Arc::new(Worker::new(
            pool.clone(),
            dispatcher.clone(),
            &worker_config,
        ));
        let runner = Arc::clone(&worker);
        workers.push(worker);
        handles.push(tokio::spawn(async move { runner.run().await }));
    }

    println!(
        "control plane ready: {} worker(s), lease {}s",
        config.max_concurrency, config.lease_seconds
    );

    // Optional API surface: opt-in via LABRYS_API_ADDR so existing
    // worker-only deployments keep their exact behavior.
    let api = match std::env::var("LABRYS_API_ADDR") {
        Ok(_) => {
            let api_config = ApiConfig::from_env()?;
            let state = ApiState::new(pool.clone(), api_config.token.clone());
            let listener = tokio::net::TcpListener::bind(api_config.bind_addr).await?;
            println!("control-plane API listening on {}", api_config.bind_addr);
            Some(tokio::spawn(async move {
                axum::serve(listener, labrys_control_plane::api_router(state)).await
            }))
        }
        Err(_) => None,
    };

    shutdown_signal().await;
    println!("shutdown requested; draining in-flight work");
    for worker in &workers {
        worker.stop();
    }
    for handle in handles {
        let _ = handle.await;
    }
    if let Some(api) = api {
        api.abort();
    }
    println!("control plane stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        #[cfg(unix)]
        {
            tokio::signal::ctrl_c().await.ok();
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => stream.recv().await,
            Err(_) => None,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
