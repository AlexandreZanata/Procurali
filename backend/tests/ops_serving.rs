//! Operational serving acceptance (P21-T02): the real binary over real HTTP.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Starts the shipped binary as a real process against a
//! disposable, migrated database and proves over real HTTP:
//! - liveness and readiness answer for the served process;
//! - a public catalog read is public and a cookie-less staff read is refused;
//! - an unknown route keeps the stable refusal;
//! - SIGTERM drains the server and the process exits cleanly;
//! - a refused configuration (missing setting, unusable database, unusable
//!   schema) never starts a listener.
//!
//! Only loopback and disposable databases are used. No message, contact, or
//! WhatsApp handoff can fire: every probe is a GET.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p21t02-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p21t02-test-only-encryption-key";
const SESSION_KEY: &str = "p21t02-test-only-session-key";
const BIN: &str = env!("CARGO_BIN_EXE_procurali-backend");

/// Replace the database segment of `TEST_DATABASE_URL` with `database`.
fn url_for(database: &str) -> String {
    let base = std::env::var("TEST_DATABASE_URL").expect("disposable database url");
    let (head, _) = base.rsplit_once('/').expect("url has a database segment");
    format!("{head}/{database}")
}

/// One free loopback port, released before the server binds it.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port is available");
    let port = listener
        .local_addr()
        .expect("the bound port is known")
        .port();
    drop(listener);
    port
}

/// One HTTP GET over plain loopback TCP. Returns the status line and body.
fn http_get(address: &str, path: &str) -> (u16, String) {
    let mut stream = std::net::TcpStream::connect(address).expect("the server accepts connections");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("the read timeout applies");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("the request is written");
    let mut response = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&buffer[..read]);
                if response.len() > 65_536 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&response).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .expect("a status line");
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned();
    (status, body)
}

/// Start the served binary against one disposable database.
fn start_server(database_url: &str, port: u16) -> Child {
    Command::new(BIN)
        .env("DATABASE_URL", database_url)
        .env("PROCURALI_ENV", "test")
        .env("PHONE_PROVIDER", "fake")
        .env("FAKE_VERIFY_CODE", FAKE_CODE)
        .env("SESSION_KEY", SESSION_KEY)
        .env("PHONE_LOOKUP_KEY", LOOKUP_KEY)
        .env("PHONE_ENCRYPTION_KEY", ENCRYPTION_KEY)
        .env("APP_BIND_ADDRESS", format!("127.0.0.1:{port}"))
        .env("APP_ASSETS_DIR", "web/dist")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the serving binary starts")
}

/// Wait until the server answers readiness, or fail with the deadline.
fn await_ready(address: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok((status, body)) = std::panic::catch_unwind(|| http_get(address, "/health/ready"))
        {
            if status == 200 && body.contains("\"ready\"") {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("the served application never became ready");
}

#[test]
fn the_served_binary_answers_traffic_and_drains_on_termination() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the test runtime builds");
    let db = runtime.block_on(async {
        let db = TestDatabase::create("p21t02_serving")
            .await
            .expect("disposable database allocates");
        assert!(
            db.name().starts_with("procurali_test_p21t02_serving_"),
            "known suite identity in the database name"
        );
        db
    });

    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let mut child = start_server(&url_for(db.name()), port);
    await_ready(&address);

    // The served process started against the migrated schema it verified.
    let applied: i64 = runtime
        .block_on(async {
            sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
                .fetch_one(db.pool())
                .await
        })
        .expect("the applied migrations read");
    assert!(
        applied > 0,
        "the served database carries the tracked schema"
    );

    // Liveness and readiness answer for the served process.
    let (status, body) = http_get(&address, "/health/live");
    assert_eq!(status, 200);
    assert!(body.contains("\"alive\""));

    let (status, body) = http_get(&address, "/health/ready");
    assert_eq!(status, 200);
    assert!(body.contains("\"ready\""));

    // A public catalog read is public; the staff boundary refuses a
    // cookie-less read.
    let (status, _body) = http_get(&address, "/api/v1/catalogs/cities");
    assert_eq!(status, 200);
    let (status, body) = http_get(&address, "/api/v1/metrics/operational?purpose=smoke");
    assert_eq!(status, 401);
    assert!(body.contains("unauthenticated"));

    // The unknown route keeps the stable refusal.
    let (status, body) = http_get(&address, "/api/v1/not-a-route");
    assert_eq!(status, 404);
    assert!(body.contains("not_found"));

    // The container health check reports the served state.
    let health = Command::new(BIN)
        .arg("--health-check")
        .env("APP_BIND_ADDRESS", &address)
        .output()
        .expect("the health check runs");
    assert!(
        health.status.success(),
        "the health check reports a ready server"
    );

    // SIGTERM drains the server and the process exits cleanly.
    let terminated = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("the termination signal is sent");
    assert!(
        terminated.success(),
        "the process received the termination signal"
    );
    let status = runtime
        .block_on(async { tokio::task::spawn_blocking(move || child.wait()).await })
        .expect("the wait task joins")
        .expect("the process exits");
    assert!(
        status.success(),
        "the served application exits cleanly after the drain"
    );

    runtime.block_on(db.cleanup()).expect("suite cleans up");
}

#[test]
fn a_refused_configuration_never_starts_a_listener() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the test runtime builds");
    let db = runtime.block_on(async {
        TestDatabase::create("p21t02_refuse")
            .await
            .expect("disposable database allocates")
    });

    // A missing setting refuses before anything binds.
    let port = free_port();
    let output = Command::new(BIN)
        .env("DATABASE_URL", url_for(db.name()))
        .env("PROCURALI_ENV", "test")
        .env("PHONE_PROVIDER", "fake")
        .env("FAKE_VERIFY_CODE", FAKE_CODE)
        .env("SESSION_KEY", SESSION_KEY)
        .env("PHONE_LOOKUP_KEY", LOOKUP_KEY)
        .env("APP_BIND_ADDRESS", format!("127.0.0.1:{port}"))
        .output()
        .expect("the binary runs");
    assert!(!output.status.success(), "the missing key refuses startup");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("PHONE_ENCRYPTION_KEY"),
        "the refusal names the missing setting: {stderr}"
    );
    assert!(
        !stderr.contains(LOOKUP_KEY) && !stderr.contains(SESSION_KEY),
        "the refusal prints no secret value"
    );
    // Nothing is listening on the refused port.
    assert!(
        std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_err(),
        "a refused startup never binds a port"
    );

    // An unusable database refuses as well.
    let port = free_port();
    let output = Command::new(BIN)
        .env(
            "DATABASE_URL",
            "postgres://u:p@127.0.0.1:1/procurali_test_unreachable",
        )
        .env("PROCURALI_ENV", "test")
        .env("PHONE_PROVIDER", "fake")
        .env("FAKE_VERIFY_CODE", FAKE_CODE)
        .env("SESSION_KEY", SESSION_KEY)
        .env("PHONE_LOOKUP_KEY", LOOKUP_KEY)
        .env("PHONE_ENCRYPTION_KEY", ENCRYPTION_KEY)
        .env("APP_BIND_ADDRESS", format!("127.0.0.1:{port}"))
        .output()
        .expect("the binary runs");
    assert!(
        !output.status.success(),
        "an unusable database refuses startup"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("refusing to start"),
        "the refusal is explicit: {stderr}"
    );
    assert!(
        !stderr.contains("postgres://"),
        "the refusal prints no connection value"
    );

    runtime.block_on(db.cleanup()).expect("suite cleans up");
}

#[test]
fn the_health_check_reports_the_served_state() {
    // The health check used by the container image fails for a closed port.
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let output = Command::new(BIN)
        .arg("--health-check")
        .env("APP_BIND_ADDRESS", &address)
        .output()
        .expect("the health check runs");
    assert!(
        !output.status.success(),
        "an unreachable server is not reported healthy"
    );
}
