//! The migration job: apply the tracked migrations and exit.
//!
//! Migrations stay a separate, explicitly authorized step: the serving
//! binary never applies them. This job is that step, packaged with the
//! application so an operator (or a disposable harness) does not need a
//! separate toolchain. It reads one setting, applies the embedded journal
//! exactly once, and prints a redacted line.
//!
//! Refusals name the reason and print no value: a missing or non-postgres
//! `DATABASE_URL` and an unreachable database all exit with the refusal
//! code.

use procurali_backend::persistence::migrations;
use procurali_backend::persistence::pool::{self, PoolConfig};

/// Exit code for a refused migration job.
const REFUSED: u8 = 2;

fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("procurali-migrate: refusing to run: async runtime unavailable");
            return std::process::ExitCode::from(REFUSED);
        }
    };
    match runtime.block_on(run()) {
        Ok(applied) => {
            println!("procurali-migrate: applied {applied} migration(s)");
            std::process::ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("procurali-migrate: refusing to run: {message}");
            std::process::ExitCode::from(REFUSED)
        }
    }
}

/// Apply every pending migration, then report how many the journal holds.
async fn run() -> Result<usize, String> {
    let url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "missing required setting: DATABASE_URL".to_owned())?;
    if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
        return Err("invalid setting: DATABASE_URL".to_owned());
    }
    let pool = pool::connect(&url, &PoolConfig::default())
        .await
        .map_err(|error| error.to_string())?;
    migrations::apply(&pool)
        .await
        .map_err(|_| "the migration job failed".to_owned())?;
    let applied = migrations::MIGRATOR.iter().count();
    pool.close().await;
    Ok(applied)
}
