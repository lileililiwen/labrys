use std::sync::Arc;

use labrys_control_plane::{connect, run_migrations, Config, NoopDispatcher, Worker};

/// Control-plane process entry point.
///
/// Loads database credentials and runtime settings from process configuration,
/// runs embedded migrations explicitly, and starts the reconciliation workers.
/// Provider/runtime execution is the safe no-op dispatcher until container
/// execution lands in a later change, so this process never performs real
/// runtime work.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let pool = connect(&config).await?;
    if config.run_migrations {
        run_migrations(&pool).await?;
        println!("migrations applied");
    }

    let dispatcher = Arc::new(NoopDispatcher);
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

    shutdown_signal().await;
    println!("shutdown requested; draining in-flight work");
    for worker in &workers {
        worker.stop();
    }
    for handle in handles {
        let _ = handle.await;
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
