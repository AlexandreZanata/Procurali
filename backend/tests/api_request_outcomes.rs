//! Closure acceptance (P06-T06): declared buyer outcomes.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). The closure operation currently has no HTTP route (the
//! route-owning card lands it later), so these tests drive the application
//! operation directly over real PostgreSQL — the same direct-proof pattern
//! as the publication and renewal cards. Proves:
//! - strangers cannot close, and drafts refuse closure;
//! - completed, cancelled, and expired facts stay distinguishable with
//!   repeats recording nothing;
//! - a suspended buyer completes their own request with visibility
//!   untouched and no fictional contact.
//!
//! Setup publishes travel the real draft and publication operations. All
//! phones, names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::close_request::{
    close_request, BuyerOutcome, CloseError, DeclaredOutcome, OutcomeSource,
};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::record_outcome::{
    record_outcome, CompletionSource, OutcomeAnswer, OutcomeError,
};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::request_eligibility::expire_if_elapsed;
use procurali_backend::application::start_contact::{start_contact, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::requests::{cycles_for_request, request as read_request};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

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

/// A real account, activated through a synthetic state fixture: identity
/// itself travels the real creation path with real phone cryptography, and
/// only the lifecycle flip (owned by the verification flow) is staged.
async fn seed_author(pool: &sqlx::PgPool, display: &str, phone: &str) -> uuid::Uuid {
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
            lookup_key: "p06t06-test-only-lookup-key",
            encryption_key: "p06t06-test-only-encryption-key",
        },
    )
    .await
    .expect("fixture author stores");
    tx.commit().await.expect("author commits");
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(user.id)
        .execute(pool)
        .await
        .expect("synthetic activation applies");
    user.id
}

/// One genuinely open request through the real draft and publication paths.
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("520.00".to_owned()),
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

async fn request_kinds(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT kind, payload::text FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1
         ORDER BY occurred_at, id",
    )
    .bind(request_id)
    .fetch_all(pool)
    .await
    .expect("events read")
}

#[tokio::test]
async fn stranger_cannot_close_need_and_drafts_refuse_closure() {
    let db = TestDatabase::create("p06t06_stranger")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t06_stranger_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Close Owner", "+55 11 90000-0091").await;
    let stranger = seed_author(db.pool(), "Close Stranger", "+55 11 90000-0092").await;
    let id = publish_open(db.pool(), author, "Refrigerator").await;

    // Closure is owner-only: strangers share the missing-row refusal, and
    // the row stands untouched with no fact recorded.
    for outcome in [
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
        BuyerOutcome::NotNeeded,
        BuyerOutcome::NotYet,
    ] {
        assert_eq!(
            close_request(db.pool(), stranger, id, outcome).await,
            Err(CloseError::NotFound)
        );
    }
    assert_eq!(
        close_request(
            db.pool(),
            stranger,
            uuid::Uuid::now_v7(),
            BuyerOutcome::NotNeeded
        )
        .await,
        Err(CloseError::NotFound)
    );
    let stored = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(stored.state, "active");
    let kinds = request_kinds(db.pool(), id).await;
    assert!(
        !kinds
            .iter()
            .any(|(kind, _)| kind == "request.completed" || kind == "request.cancelled"),
        "stranger attempts record no closure fact"
    );

    // Drafts close through draft editing, never through closure.
    let draft = create_draft(
        db.pool(),
        author,
        DraftInput {
            title: Some("Spare Fridge".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("100.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("fixture draft validates");
    assert_eq!(
        close_request(db.pool(), author, draft.id, BuyerOutcome::NotNeeded).await,
        Err(CloseError::ForbiddenState)
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn completed_cancelled_and_expired_facts_stay_distinguishable() {
    let db = TestDatabase::create("p06t06_facts")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t06_facts_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Facts Owner", "+55 11 90000-0093").await;

    // "Yes, I found it" completes with the declared source, recorded as
    // declared — never as a verified sale.
    let found = publish_open(db.pool(), author, "Found Fridge").await;
    let closed = close_request(
        db.pool(),
        author,
        found,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("completion closes");
    assert_eq!(closed.state, "completed");
    assert_eq!(
        closed.outcome,
        DeclaredOutcome::Completed(OutcomeSource::Elsewhere)
    );
    let facts = request_kinds(db.pool(), found).await;
    assert!(facts.iter().any(|(kind, _)| kind == "request.published"));
    let completed = facts
        .iter()
        .find(|(kind, _)| kind == "request.completed")
        .expect("completion fact records");
    assert!(completed.1.contains("elsewhere"), "source stays declared");
    assert!(!facts.iter().any(|(kind, _)| kind == "request.cancelled"));

    // "I no longer need it" cancels for buyer abandonment — neither expiry
    // nor resolution.
    let dropped = publish_open(db.pool(), author, "Dropped Fridge").await;
    let closed = close_request(db.pool(), author, dropped, BuyerOutcome::NotNeeded)
        .await
        .expect("cancellation closes");
    assert_eq!(closed.state, "cancelled");
    assert_eq!(closed.outcome, DeclaredOutcome::Cancelled);
    let facts = request_kinds(db.pool(), dropped).await;
    let cancelled = facts
        .iter()
        .find(|(kind, _)| kind == "request.cancelled")
        .expect("cancellation fact records");
    assert!(
        cancelled.1.contains("buyer_abandonment"),
        "reason stays recorded"
    );
    assert!(!facts.iter().any(|(kind, _)| kind == "request.completed"));

    // Expiry is a third distinct fact on a third distinct state.
    let lapsed = publish_open(db.pool(), author, "Lapsed Fridge").await;
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(lapsed)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic expiry applies");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    expire_if_elapsed(&mut tx, lapsed, now)
        .await
        .expect("row expires");
    tx.commit().await.expect("transition commits");
    let facts = request_kinds(db.pool(), lapsed).await;
    assert!(facts.iter().any(|(kind, _)| kind == "request.expired"));
    assert!(!facts
        .iter()
        .any(|(kind, _)| { kind == "request.completed" || kind == "request.cancelled" }));

    // Terminal rows stay terminal: repeats record nothing further, and both
    // closed cycles ended instead of lingering.
    for (id, outcome) in [
        (found, BuyerOutcome::Found(OutcomeSource::Platform)),
        (dropped, BuyerOutcome::NotNeeded),
    ] {
        assert_eq!(
            close_request(db.pool(), author, id, outcome).await,
            Err(CloseError::ForbiddenState)
        );
    }
    assert_eq!(request_kinds(db.pool(), found).await.len(), 2);
    assert_eq!(request_kinds(db.pool(), dropped).await.len(), 2);
    for id in [found, dropped] {
        let cycles = cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read");
        assert_eq!(cycles.len(), 1);
        assert!(cycles[0].ended_at.is_some(), "closure ends the live cycle");
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn suspended_buyer_completes_without_restoring_contact() {
    let db = TestDatabase::create("p06t06_suspended")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t06_suspended_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Suspended Owner", "+55 11 90000-0094").await;

    // A suspended buyer completes their own request with the restriction
    // exactly as it was — including hidden — and no contact appears from
    // anywhere: only publication and completion facts exist.
    for (title, visibility) in [("Public Pause", "public"), ("Hidden Pause", "hidden")] {
        let id = publish_open(db.pool(), author, title).await;
        sqlx::query("UPDATE requests SET state = 'suspended', visibility = $2 WHERE id = $1")
            .bind(id)
            .bind(visibility)
            .execute(db.pool())
            .await
            .expect("synthetic suspension applies");
        let closed = close_request(
            db.pool(),
            author,
            id,
            BuyerOutcome::Found(OutcomeSource::Undisclosed),
        )
        .await
        .expect("suspended buyer completes");
        assert_eq!(closed.state, "completed");
        let stored = read_request(db.pool(), id)
            .await
            .expect("request reads")
            .expect("request reads");
        assert_eq!(stored.visibility, visibility, "restriction untouched");
        let kinds: Vec<String> = request_kinds(db.pool(), id)
            .await
            .into_iter()
            .map(|(kind, _)| kind)
            .collect();
        assert_eq!(kinds, ["request.published", "request.completed"]);
    }
    db.cleanup().await.expect("suite cleans up");
}

fn outcome_keys() -> PhoneKeys<'static> {
    PhoneKeys {
        lookup_key: "p06t06-test-only-lookup-key",
        encryption_key: "p06t06-test-only-encryption-key",
    }
}

async fn outcome_rows(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Vec<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT outcome, source FROM request_outcomes
         WHERE request_id = $1 ORDER BY created_at, id",
    )
    .bind(request_id)
    .fetch_all(pool)
    .await
    .expect("outcomes read")
}

/// Publish one either/600 demand plus one used/520 offer through the real
/// paths; return both ids.
async fn demand_with_offer(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    seller: uuid::Uuid,
    title: &str,
) -> (uuid::Uuid, uuid::Uuid) {
    let request_id = publish_open(pool, author, title).await;
    let offer_id = submit_offer(
        pool,
        seller,
        request_id,
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
        },
    )
    .await
    .expect("fixture submission submits")
    .id;
    (request_id, offer_id)
}

async fn contact_once(
    pool: &sqlx::PgPool,
    buyer: uuid::Uuid,
    request_id: uuid::Uuid,
    offer_id: uuid::Uuid,
) {
    start_contact(
        pool,
        &outcome_keys(),
        buyer,
        request_id,
        offer_id,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("fixture handoff starts");
}

#[tokio::test]
async fn completion_without_offers_credits_no_seller() {
    let db = TestDatabase::create("p09t01_solo")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t01_solo_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Solo Owner", "+55 11 90000-0301").await;
    let id = publish_open(db.pool(), author, "Refrigerator").await;

    // Completion with no offer in play records an unattributed outcome and
    // completes the row; repeating it returns the same row with nothing new.
    let recorded = record_outcome(
        db.pool(),
        author,
        id,
        OutcomeAnswer::Completed(CompletionSource::Elsewhere),
    )
    .await
    .expect("unattributed completion records");
    assert_eq!(recorded.outcome, "completed");
    assert_eq!(recorded.source.as_deref(), Some("elsewhere"));
    assert_eq!(recorded.attributed_offer_id, None);
    assert_eq!(recorded.supersedes, None);
    let stored = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(stored.state, "completed");
    let repeat = record_outcome(
        db.pool(),
        author,
        id,
        OutcomeAnswer::Completed(CompletionSource::Elsewhere),
    )
    .await
    .expect("repeat returns standing outcome");
    assert_eq!(repeat.id, recorded.id);
    assert_eq!(outcome_rows(db.pool(), id).await.len(), 1);
    // Nothing exists to credit: no offers, no contacts, no sellers touched.
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM offers")
            .fetch_one(db.pool())
            .await
            .expect("offers read"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM contacts")
            .fetch_one(db.pool())
            .await
            .expect("contacts read"),
        0
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn uncontacted_foreign_attribution_is_refused() {
    let db = TestDatabase::create("p09t01_credit")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t01_credit_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Credit Owner", "+55 11 90000-0302").await;
    let seller = seed_author(db.pool(), "Credit Seller", "+55 11 90000-0303").await;
    let (request_id, offer_id) = demand_with_offer(db.pool(), author, seller, "Refrigerator").await;

    // An uncontacted offer cannot receive credit, and neither can a foreign
    // one; both refusals leave the row active with no outcome stored.
    assert_eq!(
        record_outcome(
            db.pool(),
            author,
            request_id,
            OutcomeAnswer::Completed(CompletionSource::Platform { offer_id }),
        )
        .await,
        Err(OutcomeError::AttributionRefused)
    );
    let (foreign_request, foreign_offer) =
        demand_with_offer(db.pool(), author, seller, "Spare Fridge").await;
    assert_eq!(
        record_outcome(
            db.pool(),
            author,
            request_id,
            OutcomeAnswer::Completed(CompletionSource::Platform {
                offer_id: foreign_offer,
            }),
        )
        .await,
        Err(OutcomeError::AttributionRefused)
    );
    assert!(outcome_rows(db.pool(), request_id).await.is_empty());
    // A historical contact unlocks exactly its offer: credited, recorded,
    // and completed together.
    contact_once(db.pool(), author, request_id, offer_id).await;
    let recorded = record_outcome(
        db.pool(),
        author,
        request_id,
        OutcomeAnswer::Completed(CompletionSource::Platform { offer_id }),
    )
    .await
    .expect("contacted attribution records");
    assert_eq!(recorded.attributed_offer_id, Some(offer_id));
    assert_eq!(recorded.source.as_deref(), Some("platform"));
    let _ = (foreign_request, foreign_offer);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn suspended_expired_record_while_restricted() {
    let db = TestDatabase::create("p09t01_restricted")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t01_restricted_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Restricted Owner", "+55 11 90000-0304").await;

    // A suspended row records its permitted result with visibility exactly
    // as it was: restriction respected, outcome stored.
    let suspended = publish_open(db.pool(), author, "Paused Fridge").await;
    sqlx::query("UPDATE requests SET state = 'suspended' WHERE id = $1")
        .bind(suspended)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    let recorded = record_outcome(db.pool(), author, suspended, OutcomeAnswer::Cancelled)
        .await
        .expect("suspended cancellation records");
    assert_eq!(recorded.outcome, "cancelled");
    let stored = read_request(db.pool(), suspended)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(
        (stored.state.as_str(), stored.visibility.as_str()),
        ("cancelled", "public")
    );

    // An expired row records too, and corrections chain: unresolved first,
    // then completion superseding it, then a silent repeat.
    let lapsed = publish_open(db.pool(), author, "Lapsed Fridge").await;
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(lapsed)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic expiry applies");
    let first = record_outcome(db.pool(), author, lapsed, OutcomeAnswer::Unresolved)
        .await
        .expect("unresolved records");
    assert_eq!(first.supersedes, None);
    let second = record_outcome(
        db.pool(),
        author,
        lapsed,
        OutcomeAnswer::Completed(CompletionSource::Unknown),
    )
    .await
    .expect("completion corrects");
    assert_eq!(second.supersedes, Some(first.id));
    let repeat = record_outcome(
        db.pool(),
        author,
        lapsed,
        OutcomeAnswer::Completed(CompletionSource::Unknown),
    )
    .await
    .expect("repeat returns standing outcome");
    assert_eq!(repeat.id, second.id);
    assert_eq!(outcome_rows(db.pool(), lapsed).await.len(), 2);
    db.cleanup().await.expect("suite cleans up");
}
