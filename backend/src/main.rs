//! The serving binary: validated settings, bounded pool, schema agreement,
//! one composed router, real HTTP traffic, graceful shutdown.
//!
//! Startup is refused loudly and without values when the settings are
//! invalid, the database is unreachable, or the applied schema differs from
//! the tracked migrations. Nothing here decides business rules: the router
//! is the composition root in [`procurali_backend::http::app`], and
//! PostgreSQL remains the only authority.
//!
//! The startup line is redacted by construction: it carries the environment
//! label, the bind address, and the database label — never a secret, a URL, a
//! token, or a contact destination.

use std::net::SocketAddr;
use std::sync::Arc;

use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::http::app::Application;
use procurali_backend::operations::config::{PhoneProvider, Settings};
use procurali_backend::operations::lifecycle::serve;
use procurali_backend::operations::worker::run_worker;
use procurali_backend::persistence::migrations;
use procurali_backend::persistence::pool::{self, PoolConfig};

/// Exit code for a refused startup. Distinct from a crash so an operator can
/// tell a configuration problem from a runtime failure.
const STARTUP_REFUSED: u8 = 2;

fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("procurali-backend: refusing to start: async runtime unavailable");
            return std::process::ExitCode::from(STARTUP_REFUSED);
        }
    };
    match runtime.block_on(run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("procurali-backend: refusing to start: {message}");
            std::process::ExitCode::from(STARTUP_REFUSED)
        }
    }
}

/// Serve until a termination signal arrives.
async fn run() -> Result<(), String> {
    let settings = Settings::from_env().map_err(|error| error.to_string())?;

    let address: SocketAddr = settings
        .bind_address()
        .parse()
        .map_err(|_| "APP_BIND_ADDRESS is not a valid socket address".to_owned())?;

    let provider = Arc::new(select_provider(&settings)?);

    let pool = pool::connect(settings.database_url(), &PoolConfig::default())
        .await
        .map_err(|error| error.to_string())?;
    migrations::verify_schema(&pool)
        .await
        .map_err(|error| error.to_string())?;

    if !settings.assets_dir().is_dir() {
        // The built UI is part of the free MVP, so a missing bundle is a
        // refusal in the environments that serve the public, and a warning
        // where the API is exercised on its own.
        if matches!(
            settings.environment(),
            procurali_backend::operations::config::Environment::Production
                | procurali_backend::operations::config::Environment::Staging
        ) {
            return Err("APP_ASSETS_DIR does not exist".to_owned());
        }
        eprintln!("procurali-backend: warning: APP_ASSETS_DIR does not exist (no built UI served)");
    }

    let application = Application::new(
        pool.clone(),
        provider,
        settings.phone_lookup_key().to_owned(),
        settings.phone_encryption_key().to_owned(),
        settings.public_origin().map(str::to_owned),
        settings.assets_dir().to_owned(),
    );
    let router = application.router();

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|_| "the bind address is not available".to_owned())?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(());
    });

    let worker_id = format!("worker-{}", uuid::Uuid::now_v7());
    let (worker_shutdown_tx, worker_shutdown_rx) = tokio::sync::oneshot::channel();
    let worker_pool = pool.clone();
    tokio::spawn(async move {
        run_worker(&worker_pool, &worker_id, worker_shutdown_rx).await;
    });

    // Redacted by construction: labels only, never a secret or a URL.
    println!(
        "procurali-backend serving mode={:?} address={address} database={}",
        settings.environment(),
        pool::connection_label(settings.database_url())
            .unwrap_or_else(|_| "unavailable".to_owned()),
    );

    let result = serve(listener, router, shutdown_rx).await;
    let _ = worker_shutdown_tx.send(());
    result.map_err(|_| "the HTTP server stopped unexpectedly".to_owned())
}

/// The proof provider this build can serve.
///
/// The deterministic fake is served only where the configuration already
/// allows test doubles. The live Verify provider is refused: its HTTPS
/// transport is not part of this build, and a served binary must never
/// pretend that a proof was delivered.
fn select_provider(settings: &Settings) -> Result<FakeVerifyProvider, String> {
    match settings.phone_provider() {
        PhoneProvider::Fake => {
            let code = settings
                .fake_verify_code()
                .ok_or_else(|| "FAKE_VERIFY_CODE is required".to_owned())?;
            Ok(FakeVerifyProvider::for_tests(code))
        }
        PhoneProvider::Twilio => Err(
            "PHONE_PROVIDER=twilio needs the live Verify transport, which this build does not carry"
                .to_owned(),
        ),
    }
}

/// Resolve on the first termination signal (SIGTERM or interrupt).
async fn shutdown_signal() {
    let interrupt = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use procurali_backend::operations::config::Settings;
    use std::collections::BTreeMap;

    fn settings(pairs: &[(&str, &str)]) -> Settings {
        let values: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        Settings::from_map(&values).expect("synthetic settings load")
    }

    fn base(environment: &'static str) -> Vec<(&'static str, &'static str)> {
        vec![
            ("PROCURALI_ENV", environment),
            ("DATABASE_URL", "postgres://u@127.0.0.1:5432/procurali_dev"),
            ("SESSION_KEY", "test-only-session-key"),
            ("PHONE_LOOKUP_KEY", "test-only-lookup-key"),
            ("PHONE_ENCRYPTION_KEY", "test-only-encryption-key"),
            ("PHONE_PROVIDER", "fake"),
            ("FAKE_VERIFY_CODE", "135790"),
        ]
    }

    #[test]
    fn the_fake_provider_is_served_where_doubles_are_allowed() {
        for environment in ["test", "development"] {
            let settings = settings(&base(environment));
            let provider = select_provider(&settings).expect("the fake provider is served here");
            assert_eq!(
                provider,
                FakeVerifyProvider::for_tests("135790"),
                "the served provider is the configured deterministic one"
            );
        }
    }

    #[test]
    fn the_live_provider_is_refused_without_a_transport() {
        let mut pairs = base("test");
        pairs.retain(|(key, _)| *key != "FAKE_VERIFY_CODE");
        pairs.push(("PHONE_PROVIDER", "twilio"));
        pairs.push(("TWILIO_ACCOUNT_SID", "synthetic-sid"));
        pairs.push(("TWILIO_AUTH_TOKEN", "synthetic-token"));
        pairs.push(("TWILIO_VERIFY_SERVICE_SID", "synthetic-service"));
        let settings = settings(&pairs);
        let refusal = select_provider(&settings).expect_err("the live provider is refused");
        assert!(refusal.contains("twilio"));
    }

    #[test]
    fn refusals_carry_no_value() {
        let mut pairs = base("test");
        pairs.retain(|(key, _)| *key != "FAKE_VERIFY_CODE");
        pairs.push(("SESSION_KEY", "canary-session-key"));
        let values: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        // The configuration loader refuses the missing proof code by name.
        let error = Settings::from_map(&values).expect_err("the fake provider needs its code");
        assert!(format!("{error}").contains("FAKE_VERIFY_CODE"));
        assert!(!format!("{error:?} {error}").contains("canary-session-key"));
    }
}
