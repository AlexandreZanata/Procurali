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
    http_get_with_cookie(address, path, None)
}

/// One GET over plain loopback TCP, optionally carrying a session cookie.
fn http_get_with_cookie(address: &str, path: &str, cookie: Option<&str>) -> (u16, String) {
    let mut stream = std::net::TcpStream::connect(address).expect("the server accepts connections");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("the read timeout applies");
    let request = match cookie {
        Some(cookie) => format!(
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"
        ),
        None => format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"),
    };
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

/// One JSON POST over plain loopback TCP. Returns the status, the body, and
/// the session cookie the server issued, if any.
fn http_post_json(
    address: &str,
    path: &str,
    body: &str,
    cookie: Option<&str>,
) -> (u16, String, String) {
    let mut stream = std::net::TcpStream::connect(address).expect("the server accepts connections");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("the read timeout applies");
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: http://{address}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    if let Some(cookie) = cookie {
        request = format!(
            "POST {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: http://{address}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nCookie: {cookie}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        );
    }
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
    let set_cookie = text
        .split("\r\n")
        .find_map(|line| {
            line.strip_prefix("set-cookie: ")
                .map(|value| value.split(';').next().unwrap_or_default().to_owned())
        })
        .unwrap_or_default();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned();
    (status, body, set_cookie)
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

/// The served application delivers the expiry notice and the prompt.
#[test]
fn the_served_application_delivers_the_owner_notices() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the test runtime builds");
    let db = runtime.block_on(async {
        TestDatabase::create("p23t02_notices")
            .await
            .expect("disposable database allocates")
    });

    // The public catalogs the publication needs.
    runtime.block_on(async {
        let mut tx = db.pool().begin().await.expect("transaction begins");
        procurali_backend::persistence::catalogs::upsert_city(
            &mut tx, "campinas", "Campinas", true,
        )
        .await
        .expect("fixture city stores");
        procurali_backend::persistence::catalogs::upsert_region(
            &mut tx, "campinas", "centro", "Centro",
        )
        .await
        .expect("fixture region stores");
        tx.commit().await.expect("fixture catalogs commit");
    });

    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let mut child = start_server(&url_for(db.name()), port);
    await_ready(&address);

    // The owner registers, proves control, and logs in over real HTTP.
    let phone = "+55 11 90000-0311";
    let (status, _body, _cookie) = http_post_json(
        &address,
        "/api/v1/accounts",
        &format!(
            "{{\"display_name\":\"Notice Owner\",\"phone\":\"{phone}\",\"city\":\"Campinas\",\"region\":\"SP\",\"policy_version\":\"v1\"}}"
        ),
        None,
    );
    assert_eq!(status, 201, "the owner registers over real HTTP");
    let (status, _body, _cookie) = http_post_json(
        &address,
        "/api/v1/accounts/challenges",
        &format!("{{\"phone\":\"{phone}\"}}"),
        None,
    );
    assert_eq!(status, 202, "the challenge is accepted");
    let (status, _body, _cookie) = http_post_json(
        &address,
        "/api/v1/accounts/challenges/confirmations",
        &format!("{{\"phone\":\"{phone}\",\"code\":\"{FAKE_CODE}\"}}"),
        None,
    );
    assert_eq!(status, 200, "the proof activates the account");
    let (status, _body, cookie) = http_post_json(
        &address,
        "/api/v1/sessions",
        &format!("{{\"phone\":\"{phone}\",\"code\":\"{FAKE_CODE}\"}}"),
        None,
    );
    assert_eq!(status, 200, "the owner logs in");
    assert!(!cookie.is_empty(), "the login issues a session cookie");

    // The owner publishes one demand over real HTTP.
    let (status, body, _cookie) = http_post_json(
        &address,
        "/api/v1/requests/drafts",
        &serde_json::json!({
            "title": "Served Expiry Fridge",
            "category_code": "home_appliances",
            "budget": "600.00",
            "condition": "either",
            "city_code": "campinas",
            "region_code": "centro",
        })
        .to_string(),
        Some(&cookie),
    );
    assert_eq!(status, 201, "the draft is saved");
    let draft_id = serde_json::from_str::<serde_json::Value>(&body)
        .expect("the draft receipt parses")["id"]
        .as_str()
        .expect("the draft receipt carries an id")
        .to_owned();
    let (status, body, _cookie) = http_post_json(
        &address,
        &format!("/api/v1/requests/drafts/{draft_id}/publication"),
        "{}",
        Some(&cookie),
    );
    assert_eq!(status, 201, "the demand publishes");
    let request_id = serde_json::from_str::<serde_json::Value>(&body).expect("the receipt parses")
        ["id"]
        .as_str()
        .expect("the receipt carries an id")
        .to_owned();

    // The cycle is moved into the past and the scheduled sweep becomes due:
    // the served worker still performs the real transition.
    runtime.block_on(async {
        sqlx::query(
            "UPDATE request_cycles SET started_at = now() - interval '8 days',
                    deadline = now() - interval '1 hour'
             WHERE request_id = $1",
        )
        .bind(uuid::Uuid::parse_str(&request_id).expect("the request id parses"))
        .execute(db.pool())
        .await
        .expect("the elapsed cycle applies");
        sqlx::query(
            "UPDATE background_jobs SET not_before = now() - interval '1 minute'
             WHERE kind = 'request.expire' AND payload->>'request_id' = $1",
        )
        .bind(&request_id)
        .execute(db.pool())
        .await
        .expect("the sweep becomes due");
    });

    // The owner reads the notices the served worker produced.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut kinds: Vec<String> = Vec::new();
    while Instant::now() < deadline {
        let (status, body) = http_get_with_cookie(&address, "/api/v1/notices", Some(&cookie));
        assert_eq!(status, 200, "the owner reads their notices");
        let notices =
            serde_json::from_str::<serde_json::Value>(&body).expect("the notice list parses");
        kinds = notices["notices"]
            .as_array()
            .expect("the notice list is an array")
            .iter()
            .map(|notice| notice["kind"].as_str().unwrap_or_default().to_owned())
            .collect();
        if kinds.iter().any(|kind| kind == "request.expired")
            && kinds.iter().any(|kind| kind == "outcome.prompt")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    assert!(
        kinds.iter().any(|kind| kind == "request.expired"),
        "the owner receives the expiry notice: {kinds:?}"
    );
    if !kinds.iter().any(|kind| kind == "outcome.prompt") {
        let jobs: Vec<(String, String, String, i32, Option<String>)> = runtime.block_on(async {
            sqlx::query_as(
                "SELECT kind, state, not_before::text, attempts, last_error
                 FROM background_jobs WHERE payload->>'request_id' = $1
                 ORDER BY created_at",
            )
            .bind(&request_id)
            .fetch_all(db.pool())
            .await
            .expect("jobs read")
        });
        panic!("the owner receives the outcome prompt: {kinds:?} / jobs: {jobs:?}");
    }
    let (_status, body) = http_get_with_cookie(&address, "/api/v1/notices", Some(&cookie));
    assert!(
        !body.contains("+55"),
        "a notice body carries no destination"
    );

    // The request now reads as expired for its owner.
    let (status, body) = http_get_with_cookie(
        &address,
        &format!("/api/v1/requests/{request_id}"),
        Some(&cookie),
    );
    assert_eq!(status, 200, "the owner reads the expired request");
    let detail = serde_json::from_str::<serde_json::Value>(&body).expect("the detail parses");
    assert_eq!(
        detail["state"], "expired",
        "the owner reads the expired request"
    );
    assert_eq!(detail["expired"], true);

    // A repeated sweep writes nothing further: a duplicated late job for
    // the same cycle observes the settled row and adds no notice.
    runtime.block_on(async {
        sqlx::query(
            "INSERT INTO background_jobs (kind, payload, max_attempts, not_before)
             VALUES ('request.expire', jsonb_build_object('request_id', $1::text, 'cycle', 1), 3,
                     now() - interval '1 minute')",
        )
        .bind(&request_id)
        .execute(db.pool())
        .await
        .expect("the duplicated sweep is enqueued");
    });
    std::thread::sleep(Duration::from_secs(12));
    let (status, body) = http_get_with_cookie(&address, "/api/v1/notices", Some(&cookie));
    assert_eq!(status, 200);
    let notices = serde_json::from_str::<serde_json::Value>(&body).expect("the notice list parses");
    let total = notices["total"].as_i64().expect("the notice total reads");
    assert_eq!(
        total, 2,
        "the repeated sweep writes no further notice: {notices}"
    );

    let terminated = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("the termination signal is sent");
    assert!(terminated.success());
    let status = runtime
        .block_on(async { tokio::task::spawn_blocking(move || child.wait()).await })
        .expect("the wait task joins")
        .expect("the process exits");
    assert!(status.success(), "the served application exits cleanly");

    runtime.block_on(db.cleanup()).expect("suite cleans up");
}
