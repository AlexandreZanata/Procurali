//! E2E harness seeder (P22-T01): the permanent harness state.
//!
//! The browser suite reads a fixtures file and expects harness accounts that
//! already exist. This suite produces exactly that state on one explicitly
//! disposable database, and it reaches every fact through the real business
//! operations — the same operations the served application uses:
//!
//! - the public catalogs (the specs publish demands in `campinas/centro`);
//! - the harness accounts through the real registration, challenge, and
//!   confirmation flow with the deterministic fake provider;
//! - the staff standing the moderation specs need (an audited grant);
//! - one published demand with a withdrawn offer on it (the withdrawn slot
//!   stays consumed);
//! - one suspended demand (hidden for strangers, still owned).
//!
//! The fixtures file carries identifiers only: no credential, token,
//! destination, or private fact ever reaches it.
//!
//! It runs only when the harness environment names a disposable database:
//! `E2E_HARNESS_DATABASE_URL` (database name must start with
//! `procurali_test_`, host must be loopback) and `E2E_FIXTURES_PATH` (where
//! the fixtures are written). A missing or unsafe input fails loudly — it
//! never skips and never touches a non-disposable database.

use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::register_user::{
    confirm_challenge, register_account, start_challenge, ConfirmationOutcome,
};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::staff_permissions::{bootstrap_grant, BootstrapInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::suspend_content::{suspend_content, SuspendInput};
use procurali_backend::application::withdraw_offer::{withdraw_offer, WithdrawReason};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::pool::{connect, PoolConfig};
use procurali_backend::persistence::users::{canonicalize_phone, find_user_by_phone, PhoneKeys};
use sqlx::PgPool;

/// The deterministic proof code of the disposable fake provider.
const FAKE_CODE: &str = "135790";

/// Harness accounts the browser suite logs in with.
const BUYER_PHONE: &str = "+55 11 90000-3101";
const SELLER_PHONE: &str = "+55 11 90000-3102";
const MODERATOR_PHONE: &str = "+55 11 90000-3104";

/// Harness demand titles (the fixtures carry their identifiers, not names).
const RETIRING_TITLE: &str = "E2E Harness Retiring Fridge";
const SUSPENDED_TITLE: &str = "E2E Harness Suspended Fridge";

/// The phone identity keys of the served application. The harness must use
/// the same keys as the server it seeds, or the server cannot resolve the
/// harness numbers.
static PHONE_KEYS: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

fn key_pair() -> &'static (String, String) {
    PHONE_KEYS.get_or_init(|| {
        (
            std::env::var("PHONE_LOOKUP_KEY")
                .expect("PHONE_LOOKUP_KEY is required (the served application uses the same keys)"),
            std::env::var("PHONE_ENCRYPTION_KEY").expect(
                "PHONE_ENCRYPTION_KEY is required (the served application uses the same keys)",
            ),
        )
    })
}

fn keys() -> PhoneKeys<'static> {
    let (lookup_key, encryption_key) = key_pair();
    PhoneKeys {
        lookup_key: lookup_key.as_str(),
        encryption_key: encryption_key.as_str(),
    }
}

/// The disposable database URL, refused unless it is explicitly disposable
/// and loopback.
fn harness_database_url() -> String {
    let url = std::env::var("E2E_HARNESS_DATABASE_URL")
        .expect("E2E_HARNESS_DATABASE_URL is required (explicit disposable naming)");
    validate_harness_url(&url);
    url
}

/// Refuse any database that is not explicitly disposable and loopback.
///
/// The harness writes business facts: it must never touch a shared or
/// production database, and it must never reach outside the workstation.
fn validate_harness_url(url: &str) {
    let (scheme, rest) = url
        .split_once("://")
        .expect("E2E_HARNESS_DATABASE_URL must be a postgres URL");
    assert!(
        scheme == "postgres" || scheme == "postgresql",
        "E2E_HARNESS_DATABASE_URL must use the postgres scheme"
    );
    let authority = rest.split('/').next().unwrap_or_default().to_owned();
    let hostport = authority.rsplit('@').next().unwrap_or_default();
    let host = hostport.split(':').next().unwrap_or_default();
    assert!(
        host == "127.0.0.1" || host == "localhost",
        "E2E_HARNESS_DATABASE_URL must name a loopback host"
    );
    let database = rest
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default();
    assert!(
        database.starts_with("procurali_test_"),
        "E2E_HARNESS_DATABASE_URL must name a disposable procurali_test_* database"
    );
}

/// Where the fixtures are written.
fn fixtures_path() -> std::path::PathBuf {
    let path = std::env::var("E2E_FIXTURES_PATH")
        .expect("E2E_FIXTURES_PATH is required (the browser suite reads it)");
    std::path::PathBuf::from(path)
}

/// One active harness account, created through the real proof flow.
async fn harness_account(pool: &PgPool, display: &str, phone: &str) -> uuid::Uuid {
    let provider = FakeVerifyProvider::for_tests(FAKE_CODE);
    let canonical = canonicalize_phone(phone).expect("harness phone is valid");
    if let Some(existing) = find_user_by_phone(pool, &canonical, keys().lookup_key)
        .await
        .expect("harness account lookup")
    {
        if existing.state == "active" {
            return existing.id;
        }
    }
    register_account(
        pool,
        procurali_backend::persistence::users::NewUser {
            display_name: display.to_owned(),
            city: "Campinas".to_owned(),
            region: "SP".to_owned(),
            policy_version: "v1".to_owned(),
            policy_accepted_at: chrono::Utc::now(),
            phone: phone.to_owned(),
        },
        keys(),
    )
    .await
    .expect("harness account registers");
    start_challenge(
        pool,
        &provider,
        phone,
        keys(),
        std::time::Duration::from_secs(600),
    )
    .await
    .expect("harness challenge dispatches");
    let outcome = confirm_challenge(pool, &provider, phone, FAKE_CODE, keys())
        .await
        .expect("harness confirmation answers");
    assert!(
        matches!(
            outcome,
            ConfirmationOutcome::Activated | ConfirmationOutcome::AlreadyActive
        ),
        "the harness account activates through the real proof flow"
    );
    let active = find_user_by_phone(pool, &canonical, keys().lookup_key)
        .await
        .expect("harness account lookup")
        .expect("the harness account exists");
    assert_eq!(active.state, "active");
    active.id
}

/// One published harness demand, reused when a previous run left it.
async fn harness_demand(pool: &PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
    let existing: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM requests WHERE author_id = $1 AND title = $2")
            .bind(author)
            .bind(title)
            .fetch_optional(pool)
            .await
            .expect("harness demand lookup");
    if let Some(id) = existing {
        return id;
    }
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("600.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("harness draft saves");
    let published = publish_request(pool, author, draft.id)
        .await
        .expect("harness demand publishes");
    published.id
}

#[tokio::test]
async fn harness_seeds_the_browser_fixtures() {
    let url = harness_database_url();
    let pool = connect(&url, &PoolConfig::default())
        .await
        .expect("the disposable database answers");

    // Public catalogs: the specs publish demands in this locality.
    let mut tx = pool.begin().await.expect("transaction begins");
    upsert_city(&mut tx, "campinas", "Campinas", true)
        .await
        .expect("harness city stores");
    upsert_region(&mut tx, "campinas", "centro", "Centro")
        .await
        .expect("harness region stores");
    tx.commit().await.expect("harness catalogs commit");

    let buyer = harness_account(&pool, "E2E Harness Buyer", BUYER_PHONE).await;
    let seller = harness_account(&pool, "E2E Harness Seller", SELLER_PHONE).await;
    let moderator = harness_account(&pool, "E2E Harness Moderator", MODERATOR_PHONE).await;

    // Staff standing: the moderation queue and the staff metrics need a
    // live moderator grant. The bootstrap grant is the audited path.
    bootstrap_grant(
        &pool,
        BootstrapInput {
            user_id: moderator,
            role: "moderator".to_owned(),
            scope: "safety".to_owned(),
            reason: "e2e harness moderation cover".to_owned(),
            operator_label: "e2e-harness".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("the harness moderator holds a live grant");

    // The retiring demand carries a withdrawn offer: the withdrawn slot
    // stays consumed, which is exactly what the specs assert.
    let retiring_demand = harness_demand(&pool, buyer, RETIRING_TITLE).await;
    let existing_offer: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM offers WHERE request_id = $1 AND seller_id = $2 ORDER BY created_at LIMIT 1",
    )
    .bind(retiring_demand)
    .bind(seller)
    .fetch_optional(&pool)
    .await
    .expect("harness offer lookup");
    let withdrawn_offer = match existing_offer {
        Some(id) => id,
        None => {
            let submitted = submit_offer(
                &pool,
                seller,
                retiring_demand,
                OfferInput {
                    revision_number: Some(1),
                    cycle_number: Some(1),
                    description: Some("Harness 300L".to_owned()),
                    price: Some("520.00".to_owned()),
                    condition: Some("used".to_owned()),
                    city_code: Some("campinas".to_owned()),
                    region_code: Some("centro".to_owned()),
                    notes: None,
                    available: Some(true),
                    available_in_city: Some(true),
                },
            )
            .await
            .expect("harness offer submits");
            withdraw_offer(&pool, seller, submitted.id, WithdrawReason::Withdrawn)
                .await
                .expect("harness offer withdraws");
            submitted.id
        }
    };
    let offer_state: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(withdrawn_offer)
        .fetch_one(&pool)
        .await
        .expect("harness offer state reads");
    assert_eq!(
        offer_state, "withdrawn",
        "the harness offer stands withdrawn"
    );

    // The suspended demand is hidden for strangers and stays owned.
    let suspended_demand = harness_demand(&pool, buyer, SUSPENDED_TITLE).await;
    let standing: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(suspended_demand)
            .fetch_one(&pool)
            .await
            .expect("harness demand standing reads");
    if standing.0 != "suspended" {
        let suspended = suspend_content(
            &pool,
            moderator,
            SuspendInput {
                target_kind: "request".to_owned(),
                target_id: suspended_demand,
                reason: "e2e harness suspension".to_owned(),
                policy_version: "v1".to_owned(),
                purpose: "e2e harness visibility proof".to_owned(),
                case_id: None,
            },
        )
        .await
        .expect("the harness demand suspends");
        assert!(suspended.transitioned, "the harness demand suspends once");
    }

    // The fixtures carry identifiers only.
    let fixtures = serde_json::json!({
        "retiring_demand": retiring_demand.to_string(),
        "withdrawn_offer": withdrawn_offer.to_string(),
        "suspended_demand": suspended_demand.to_string(),
    });
    let rendered = fixtures.to_string();
    assert!(
        !rendered.contains(FAKE_CODE)
            && !rendered.contains(keys().lookup_key)
            && !rendered.contains(keys().encryption_key)
            && !rendered.contains("+55"),
        "the fixtures carry no proof code, key, or destination"
    );
    let path = fixtures_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the fixtures directory exists");
    }
    std::fs::write(&path, rendered).expect("the fixtures file writes");

    // The seeded facts are observable in the database.
    let demand_state: String = sqlx::query_scalar("SELECT state FROM requests WHERE id = $1")
        .bind(retiring_demand)
        .fetch_one(&pool)
        .await
        .expect("harness demand state reads");
    assert_eq!(demand_state, "active");
    let suspended_state: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(suspended_demand)
            .fetch_one(&pool)
            .await
            .expect("harness demand standing reads");
    assert_eq!(suspended_state.0, "suspended");
    assert_eq!(suspended_state.1, "hidden");

    pool.close().await;
}

#[cfg(test)]
mod refusals {
    use super::validate_harness_url;

    #[test]
    fn a_disposable_loopback_database_is_accepted() {
        validate_harness_url("postgres://procurali_test:secret@127.0.0.1:5433/procurali_test_e2e");
        validate_harness_url(
            "postgresql://procurali_test:secret@localhost:5433/procurali_test_e2e",
        );
    }

    #[test]
    #[should_panic(expected = "disposable procurali_test_* database")]
    fn a_shared_database_is_refused() {
        validate_harness_url("postgres://procurali:secret@127.0.0.1:5432/procurali_prod");
    }

    #[test]
    #[should_panic(expected = "loopback host")]
    fn a_remote_database_is_refused() {
        validate_harness_url(
            "postgres://procurali_test:secret@db.example.com:5432/procurali_test_x",
        );
    }

    #[test]
    #[should_panic(expected = "postgres scheme")]
    fn another_database_engine_is_refused() {
        validate_harness_url("mysql://procurali_test:secret@127.0.0.1:3306/procurali_test_x");
    }
}
