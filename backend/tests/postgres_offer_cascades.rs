//! Offer-cascade acceptance (P07-T08): demand moves, live offers follow.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - material edits invalidate with resubmission matching latest terms and
//!   history intact on both sides;
//! - renewal retires the previous cycle while the fresh cycle responds,
//!   never reviving old rows;
//! - closure/submission races converge with no live offer on terminal
//!   demand and original terminal reasons untouched.
//!
//! Cascades compose here with the real lifecycle operations until the
//! wiring cards call them inside those transitions. Setup publishes and
//! submissions travel the real operations. All phones, names, and codes
//! below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::close_request::{close_request, BuyerOutcome, OutcomeSource};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::renew_request::{renew_request, RenewalConfirmation};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::request_offer_cascades::{
    cascade_request_offers, CascadeCause,
};
use procurali_backend::application::revise_request::{revise_request, ReviseInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::events::{record as record_event, NewEvent};
use procurali_backend::persistence::offers::{
    self, insert_terms, terms_for_offer, update_current_terms, TermsSnapshot,
};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};
use serde_json::json;

async fn seed_catalog(pool: &sqlx::PgPool) {
    let mut tx = pool.begin().await.expect("transaction begins");
    upsert_city(&mut tx, "campinas", "Campinas", true)
        .await
        .expect("fixture city stores");
    upsert_region(&mut tx, "campinas", "centro", "Centro")
        .await
        .expect("fixture region stores");
    tx.commit().await.expect("fixtures commit");
}

/// A real active account: identity travels the real creation path with real
/// phone cryptography, and only the lifecycle flip (owned by the
/// verification flow) is staged.
async fn seed_active(pool: &sqlx::PgPool, display: &str, phone: &str) -> uuid::Uuid {
    let mut tx = pool.begin().await.expect("transaction begins");
    let user = create_user(
        &mut tx,
        NewUser {
            display_name: display.to_owned(),
            city: "Campinas".to_owned(),
            region: "SP".to_owned(),
            policy_version: "v1".to_owned(),
            policy_accepted_at: chrono::Utc::now(),
            phone: phone.to_owned(),
        },
        PhoneKeys {
            lookup_key: "p07t08-test-only-lookup-key",
            encryption_key: "p07t08-test-only-encryption-key",
        },
    )
    .await
    .expect("fixture account stores");
    tx.commit().await.expect("account commits");
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(user.id)
        .execute(pool)
        .await
        .expect("synthetic activation applies");
    user.id
}

/// One genuinely open either/600 demand through the real paths.
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
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
    .expect("fixture draft validates");
    publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id
}

fn submit_input() -> OfferInput {
    OfferInput {
        revision_number: Some(1),
        cycle_number: Some(1),
        description: Some("Frost-free 300L".to_owned()),
        price: Some("520.00".to_owned()),
        condition: Some("used".to_owned()),
        city_code: Some("campinas".to_owned()),
        region_code: Some("centro".to_owned()),
        notes: None,
        available: Some(true),
        available_in_city: Some(true),
    }
}

async fn submit_open(
    pool: &sqlx::PgPool,
    seller: uuid::Uuid,
    request_id: uuid::Uuid,
) -> uuid::Uuid {
    submit_offer(pool, seller, request_id, submit_input())
        .await
        .expect("fixture submission submits")
        .id
}

async fn backdate_cycle(pool: &sqlx::PgPool, request_id: uuid::Uuid) {
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(request_id)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(pool)
    .await
    .expect("synthetic expiry applies");
}

async fn offer_state(pool: &sqlx::PgPool, offer_id: uuid::Uuid) -> (String, Option<String>, i32) {
    let row: (String, Option<String>, i32) =
        sqlx::query_as("SELECT state, terminal_reason, revision_number FROM offers WHERE id = $1")
            .bind(offer_id)
            .fetch_one(pool)
            .await
            .expect("offer reads");
    row
}

async fn live_offer_count(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM offers
         WHERE request_id = $1 AND state IN ('sent', 'viewed', 'contacted')",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .expect("live rows read")
}

async fn cascade(pool: &sqlx::PgPool, request_id: uuid::Uuid, cause: CascadeCause) -> u64 {
    let mut tx = pool.begin().await.expect("transaction begins");
    let moved = cascade_request_offers(&mut tx, request_id, cause)
        .await
        .expect("cascade moves");
    tx.commit().await.expect("cascade commits");
    moved
}

#[tokio::test]
async fn material_edit_and_resubmission_match_latest_requirements() {
    let db = TestDatabase::create("p07t08_material")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t08_material_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Material Owner", "+55 11 90000-0181").await;
    let seller = seed_active(db.pool(), "Material Seller", "+55 11 90000-0182").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;

    // A material revision invalidates the live offer with its recoverable
    // reason while both histories stand byte-stable.
    revise_request(
        db.pool(),
        author,
        request_id,
        ReviseInput {
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "Needs ice maker.".to_owned(),
        },
    )
    .await
    .expect("fixture revision revises");
    assert_eq!(
        cascade(db.pool(), request_id, CascadeCause::MaterialRevision).await,
        1
    );
    assert_eq!(
        offer_state(db.pool(), offer_id).await,
        (
            "invalidated".to_owned(),
            Some("material_revision".to_owned()),
            1
        )
    );

    // Explicit resubmission reuses the slot against the latest revision:
    // same row, new terms, one more counted submission, histories intact.
    let revision_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM request_revisions WHERE request_id = $1 AND revision_number = 2",
    )
    .bind(request_id)
    .fetch_one(db.pool())
    .await
    .expect("revision reads");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    sqlx::query("UPDATE offers SET revision_number = 2 WHERE id = $1")
        .bind(offer_id)
        .execute(&mut *tx)
        .await
        .expect("resubmission adopts latest");
    let snapshot = TermsSnapshot {
        description: "Frost-free 300L".to_owned(),
        price_cents: 52_000,
        condition: "used".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: "Fits the new notes.".to_owned(),
    };
    insert_terms(&mut tx, offer_id, 2, &snapshot)
        .await
        .expect("resubmission stores");
    update_current_terms(&mut tx, offer_id, 2, &snapshot)
        .await
        .expect("current promotes");
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(seller),
            resource_kind: "offer",
            resource_id: offer_id,
            cycle: Some(1),
            revision: Some(revision_id),
            effective_at: chrono::Utc::now(),
            kind: "offer.submitted",
            policy: "mvp-free",
            source: "api",
            payload: json!({"state": "sent", "resubmission": true}),
        },
    )
    .await
    .expect("resubmission fact records");
    tx.commit().await.expect("resubmission commits");
    let stored = offers::offer(db.pool(), offer_id)
        .await
        .expect("offer reads")
        .expect("offer reads");
    assert_eq!(stored.revision_number, 2);
    assert_eq!(stored.current_terms_number, 2);
    assert_eq!(
        terms_for_offer(db.pool(), offer_id)
            .await
            .expect("history reads")
            .len(),
        2
    );
    let submitted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE actor_id = $1 AND kind = 'offer.submitted'",
    )
    .bind(seller)
    .fetch_one(db.pool())
    .await
    .expect("events read");
    assert_eq!(submitted, 2, "resubmission consumes daily submission");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn renewal_retires_previous_cycle_without_revival() {
    let db = TestDatabase::create("p07t08_renewal")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t08_renewal_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Renew Owner", "+55 11 90000-0183").await;
    let seller = seed_active(db.pool(), "Renew Seller", "+55 11 90000-0184").await;
    let other = seed_active(db.pool(), "Renew Other", "+55 11 90000-0185").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;
    let old_offer = submit_open(db.pool(), seller, request_id).await;
    backdate_cycle(db.pool(), request_id).await;

    // Renewal opens cycle two; the cascade retires cycle one expired — the
    // larger budget below never revives it.
    renew_request(
        db.pool(),
        author,
        request_id,
        RenewalConfirmation {
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: String::new(),
        },
    )
    .await
    .expect("eligible renewal renews");
    assert_eq!(
        cascade(db.pool(), request_id, CascadeCause::CycleEnded).await,
        1
    );
    assert_eq!(
        offer_state(db.pool(), old_offer).await.0,
        "expired".to_owned()
    );
    // The fresh cycle responds through the normal submission path.
    let fresh = submit_offer(
        db.pool(),
        other,
        request_id,
        OfferInput {
            revision_number: Some(1),
            cycle_number: Some(2),
            ..submit_input()
        },
    )
    .await
    .expect("fresh cycle responds");
    assert_eq!(fresh.cycle_number, 2);
    assert_eq!(fresh.state, "sent");
    assert_eq!(live_offer_count(db.pool(), request_id).await, 1);
    // Stale-cycle submissions still refuse: only current demand responds.
    assert!(submit_offer(db.pool(), seller, request_id, submit_input())
        .await
        .is_err());
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closure_submission_race_leaves_no_live_offer() {
    let db = TestDatabase::create("p07t08_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t08_race_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Race Owner", "+55 11 90000-0186").await;
    let seller = seed_active(db.pool(), "Race Seller", "+55 11 90000-0187").await;
    let other = seed_active(db.pool(), "Race Other", "+55 11 90000-0188").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;
    let first = submit_open(db.pool(), seller, request_id).await;

    // A pre-withdrawn slot keeps its own terminal reason through every
    // later cascade: nothing revives or rewrites it.
    sqlx::query(
        "UPDATE offers SET state = 'withdrawn', terminal_reason = 'withdrawn' WHERE id = $1",
    )
    .bind(first)
    .execute(db.pool())
    .await
    .expect("synthetic withdrawal applies");

    // Deterministic order one: submit, then close with its cascade — the
    // cascade invalidates the just-submitted row.
    let early = submit_open(db.pool(), other, request_id).await;
    close_request(
        db.pool(),
        author,
        request_id,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("closure closes");
    cascade(db.pool(), request_id, CascadeCause::RequestClosed).await;
    assert_eq!(live_offer_count(db.pool(), request_id).await, 0);
    let invalidated = offers::offer(db.pool(), early)
        .await
        .expect("offer reads")
        .expect("offer reads");
    assert_eq!(invalidated.state, "invalidated");

    // Deterministic order two needs a fresh demand, since closure is
    // terminal: close with its cascade first, and submission refuses.
    let second = publish_open(db.pool(), author, "Refrigerator Two").await;
    close_request(
        db.pool(),
        author,
        second,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("closure closes");
    cascade(db.pool(), second, CascadeCause::RequestClosed).await;
    assert!(submit_offer(db.pool(), other, second, submit_input())
        .await
        .is_err());
    assert_eq!(live_offer_count(db.pool(), second).await, 0);

    // The barrier race between real closer and real submitter exercises the
    // remaining interleaving: because closure and its cascade commit
    // separately until the wiring card fuses them, a submission landing
    // strictly between the two commits stays briefly live — so a follow-up
    // sweep (what production scheduling performs continuously) converges
    // the row, and the test asserts that converged end state.
    let raced = publish_open(db.pool(), author, "Refrigerator Three").await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let close = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            close_request(
                &pool,
                author,
                raced,
                BuyerOutcome::Found(OutcomeSource::Elsewhere),
            )
            .await
            .expect("closure closes");
            cascade(&pool, raced, CascadeCause::RequestClosed).await;
        }
    };
    let submit = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            submit_offer(&pool, other, raced, submit_input()).await
        }
    };
    let ((), _) = tokio::join!(close(std::sync::Arc::clone(&barrier)), submit(barrier),);
    cascade(db.pool(), raced, CascadeCause::RequestClosed).await;
    assert_eq!(live_offer_count(db.pool(), raced).await, 0);
    let withdrawn = offers::offer(db.pool(), first)
        .await
        .expect("offer reads")
        .expect("offer reads");
    assert_eq!(withdrawn.state, "withdrawn");
    assert_eq!(withdrawn.terminal_reason.as_deref(), Some("withdrawn"));
    db.cleanup().await.expect("suite cleans up");
}
