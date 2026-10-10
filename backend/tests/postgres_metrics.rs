//! Cohort-metric acceptance (P14-T03) plus retention/sharing/operational
//! indicators (P14-T04): exact ratios, honest unknowns, no paid invention.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). There is no metrics HTTP surface yet, so these tests drive
//! the application operations directly over real PostgreSQL — the same
//! direct-proof pattern as the staff-grant cards ("api" here is the
//! audited Rust operation surface).
//!
//! P14-T03 proves one demand with two distinct seller contacts measures
//! North Star 2 with full coverage and no inferred sale, elsewhere and
//! unknown-source resolutions stay separate, and empty cohorts report
//! not-applicable.
//!
//! P14-T04 proves repeat and unknown visitor data never inflates unique
//! acquisition, professional declaration never implies paid custom, and
//! reviewed incident rate differs from raw volume with delay.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::create_report::ReportInput;
use procurali_backend::application::metrics::{compute_cohort, CohortScope, Maturity};
use procurali_backend::application::operational_metrics::{
    allowance_friction, compute_retention, incident_metrics, is_paid_customer, paid_indicator,
    professional_activity, radar_adoption, sharing_counts, summarize_acquisition,
    PROFESSIONAL_ACTIVITY_DAYS,
};
use procurali_backend::application::professional_profile::{declare_profile, NewProfessional};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::record_outcome::{
    record_outcome, CompletionSource, OutcomeAnswer,
};
use procurali_backend::application::report_updates::submit_grouped_report;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::share_attribution::{attribute_registration, record_landing};
use procurali_backend::application::share_request::prepare_share;
use procurali_backend::application::start_contact::{start_contact, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::view_offer::view_offer;
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p14t03-test-only-lookup-key",
        encryption_key: "p14t03-test-only-encryption-key",
    }
}

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
        test_keys(),
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

/// One published demand through the real operations.
async fn publish_demand(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
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

/// One viewed and contacted offer through the real operations.
async fn contacted_offer(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    seller: uuid::Uuid,
    demand: uuid::Uuid,
    price: &str,
) -> uuid::Uuid {
    let offer = submit_offer(
        pool,
        seller,
        demand,
        OfferInput {
            revision_number: Some(1),
            cycle_number: Some(1),
            description: Some("Frost-free 300L".to_owned()),
            price: Some(price.to_owned()),
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
    view_offer(pool, author, demand, offer)
        .await
        .expect("fixture view records");
    start_contact(
        pool,
        &test_keys(),
        author,
        demand,
        offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("fixture handoff starts");
    offer
}

fn cohort_window(
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> CohortScope {
    CohortScope {
        city_code: Some("campinas".to_owned()),
        category_code: Some("home_appliances".to_owned()),
        from,
        to,
    }
}

#[tokio::test]
async fn one_request_two_contacts_north_star_two() {
    let db = TestDatabase::create("p14t03_northstar")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t03_northstar_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "North Buyer", "+55 11 90000-2901").await;
    let seller_one = seed_active(db.pool(), "North Seller One", "+55 11 90000-2902").await;
    let seller_two = seed_active(db.pool(), "North Seller Two", "+55 11 90000-2903").await;
    let from = chrono::Utc::now() - chrono::Duration::hours(1);
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;
    contacted_offer(db.pool(), buyer, seller_one, demand, "520.00").await;
    contacted_offer(db.pool(), buyer, seller_two, demand, "480.00").await;
    let to = chrono::Utc::now() + chrono::Duration::hours(1);

    // One demand with two distinct seller contacts: North Star 2 with
    // 100% coverage, full offer ratios, zero completions — and no sale,
    // success-percentage, or trust vocabulary anywhere near the numbers.
    let metrics = compute_cohort(db.pool(), cohort_window(from, to))
        .await
        .expect("cohort computes");
    assert_eq!(metrics.published, 1);
    assert_eq!(metrics.north_star.numerator, 2);
    assert_eq!(metrics.north_star.denominator, 1);
    assert_eq!(metrics.north_star.value, Some(2.0));
    assert_eq!(metrics.coverage.value, Some(1.0));
    assert_eq!(metrics.offer_coverage.value, Some(1.0));
    assert_eq!(metrics.offers_per_request.value, Some(2.0));
    assert_eq!(metrics.offers_per_request_median, Some(2.0));
    assert_eq!(metrics.offer_view_rate.value, Some(1.0));
    assert_eq!(metrics.offer_to_contact.value, Some(1.0));
    assert_eq!(metrics.completed, 0);
    assert_eq!(metrics.unknown_outcomes, 1);
    let rendered = serde_json::to_string(&metrics).expect("metrics serialize");
    for absent in ["sale", "sold", "trust", "score", "verified"] {
        assert!(!rendered.contains(absent), "no {absent} in metrics");
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn elsewhere_differs_from_platform_attributed() {
    let db = TestDatabase::create("p14t03_attribution")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t03_attribution_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Split Buyer", "+55 11 90000-2904").await;
    let seller = seed_active(db.pool(), "Split Seller", "+55 11 90000-2905").await;
    let from = chrono::Utc::now() - chrono::Duration::hours(1);
    let platform_demand = publish_demand(db.pool(), buyer, "Platform Fridge").await;
    let platform_offer = contacted_offer(db.pool(), buyer, seller, platform_demand, "520.00").await;
    record_outcome(
        db.pool(),
        buyer,
        platform_demand,
        OutcomeAnswer::Completed(CompletionSource::Platform {
            offer_id: platform_offer,
        }),
    )
    .await
    .expect("platform outcome records");
    let elsewhere_demand = publish_demand(db.pool(), buyer, "Elsewhere Fridge").await;
    record_outcome(
        db.pool(),
        buyer,
        elsewhere_demand,
        OutcomeAnswer::Completed(CompletionSource::Elsewhere),
    )
    .await
    .expect("elsewhere outcome records");
    let undisclosed_demand = publish_demand(db.pool(), buyer, "Quiet Fridge").await;
    record_outcome(
        db.pool(),
        buyer,
        undisclosed_demand,
        OutcomeAnswer::Completed(CompletionSource::Unknown),
    )
    .await
    .expect("undisclosed outcome records");
    let to = chrono::Utc::now() + chrono::Duration::hours(1);

    // Three completions split one platform against two elsewhere-source:
    // the platform rate counts only the contact-linked finding.
    let metrics = compute_cohort(db.pool(), cohort_window(from, to))
        .await
        .expect("cohort computes");
    assert_eq!(metrics.published, 3);
    assert_eq!(metrics.completed, 3);
    assert_eq!(metrics.completed_platform, 1);
    assert_eq!(metrics.completed_elsewhere, 2);
    assert_ne!(metrics.completed_platform, metrics.completed);
    assert_eq!(metrics.resolution_rate.value, Some(1.0));
    assert_eq!(metrics.platform_resolution_rate.numerator, 1);
    assert_eq!(metrics.platform_resolution_rate.denominator, 3);
    assert_eq!(metrics.respondent_success.value, Some(1.0));
    assert_eq!(metrics.unknown_outcomes, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn empty_and_immature_are_not_false_zero() {
    let db = TestDatabase::create("p14t03_empty")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t03_empty_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Empty Buyer", "+55 11 90000-2906").await;
    let seller = seed_active(db.pool(), "Empty Seller", "+55 11 90000-2907").await;
    publish_demand(db.pool(), buyer, "Refrigerator").await;
    let now = chrono::Utc::now();

    // An empty cohort reports not-applicable everywhere instead of zeros.
    let empty = compute_cohort(
        db.pool(),
        cohort_window(
            now - chrono::Duration::days(60),
            now - chrono::Duration::days(30),
        ),
    )
    .await
    .expect("empty cohort computes");
    assert_eq!(empty.published, 0);
    assert_eq!(empty.maturity, Maturity::Mature);
    for value in [
        empty.north_star.value,
        empty.coverage.value,
        empty.offer_coverage.value,
        empty.offers_per_request.value,
        empty.offer_view_rate.value,
        empty.offer_to_contact.value,
        empty.resolution_rate.value,
        empty.platform_resolution_rate.value,
        empty.respondent_success.value,
        empty.outcome_response.value,
    ] {
        assert_eq!(value, None);
    }
    assert_eq!(empty.offers_per_request_median, None);
    assert_eq!(empty.completed, 0);
    assert_eq!(empty.unknown_outcomes, 0);

    // A fresh cohort is immature with its observation still counting: the
    // unresolved demand reads unknown, never a false zero success rate.
    let fresh = compute_cohort(
        db.pool(),
        cohort_window(
            now - chrono::Duration::hours(1),
            now + chrono::Duration::hours(1),
        ),
    )
    .await
    .expect("fresh cohort computes");
    assert_eq!(fresh.published, 1);
    assert_eq!(fresh.maturity, Maturity::Immature);
    assert_eq!(fresh.unknown_outcomes, 1);
    assert_eq!(fresh.respondent_success.value, None);
    let _ = seller;
    db.cleanup().await.expect("suite cleans up");
}

// P14-T04: repeat and unknown visitor data never inflates unique acquisition.
//
// Seeds one shareable demand, one recorded share intent, two identifiable
// landings plus one direct visit, and one attributable registration. The
// pure summarizer deduplicates a reused marker while the durable counts
// keep anonymous visits separate. Retention over two staged 30-day windows
// proves the same buyer/seller returning without draft-only inflation.
#[tokio::test]
async fn repeat_and_unknown_visits_do_not_inflate_acquisition() {
    let db = TestDatabase::create("p14t04_acquisition")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t04_acquisition_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Share Buyer", "+55 11 90000-2910").await;
    let seller = seed_active(db.pool(), "Share Seller", "+55 11 90000-2911").await;
    let demand = publish_demand(db.pool(), buyer, "Shared Refrigerator").await;
    prepare_share(db.pool(), Some(buyer), demand, "copy")
        .await
        .expect("share intent records");

    // Two identifiable landings plus one direct visit; the first marker is
    // reused once to model a repeat view of the same link.
    let first = record_landing(
        db.pool(),
        Some(demand),
        Some(chrono::Utc::now() - chrono::Duration::hours(2)),
    )
    .await
    .expect("first landing records");
    let second = record_landing(
        db.pool(),
        Some(demand),
        Some(chrono::Utc::now() - chrono::Duration::hours(1)),
    )
    .await
    .expect("second landing records");
    let _direct = record_landing(db.pool(), None, None)
        .await
        .expect("direct visit records");

    // A registration after the first landing attributes; a missing marker
    // and the direct visit stay unknown by construction.
    let newcomer = seed_active(db.pool(), "Share Newcomer", "+55 11 90000-2912").await;
    let attributed = attribute_registration(db.pool(), newcomer, Some(first.id))
        .await
        .expect("attribution reads");
    assert!(
        matches!(
            attributed,
            procurali_backend::application::share_attribution::Attribution::Attributed { .. }
        ),
        "fresh identifiable marker attributes"
    );
    let unknown = attribute_registration(db.pool(), newcomer, None)
        .await
        .expect("missing marker reads unknown");
    assert_eq!(
        unknown,
        procurali_backend::application::share_attribution::Attribution::Unknown
    );

    let counts = sharing_counts(db.pool())
        .await
        .expect("sharing counts read");
    assert_eq!(counts.share_intents, 1);
    assert_eq!(counts.total_landings, 3);
    assert_eq!(counts.identifiable_landings, 2);
    assert_eq!(counts.direct_visits, 1);

    // Repeat marker reuse deduplicates: three observations collapse to two
    // attributable visitors, with the direct visit kept out of the ratio.
    let summary = summarize_acquisition(
        &[first.id, first.id, second.id],
        &[first.id],
        counts.direct_visits,
    );
    assert_eq!(summary.attributable_visitors, 2);
    assert_eq!(summary.attributed_registrations, 1);
    assert_eq!(summary.anonymous_visits, 1);
    assert_eq!(summary.conversion.value, Some(0.5));

    // Retention fixture staging (clock backdating disclosed): the same
    // buyer publishes in both windows and the same seller supplies in both
    // windows, so both recur exactly once. The retention instant is taken
    // after all writes with a small future buffer so current-window rows
    // (half-open end) are inside the window.
    let staging_now = chrono::Utc::now();
    let previous_demand = publish_demand(db.pool(), buyer, "Previous Fridge").await;
    sqlx::query("UPDATE requests SET original_published_at = $1 WHERE id = $2")
        .bind(staging_now - chrono::Duration::days(40))
        .bind(previous_demand)
        .execute(db.pool())
        .await
        .expect("previous publication stages");
    let previous_offer = contacted_offer(db.pool(), buyer, seller, previous_demand, "500.00").await;
    sqlx::query("UPDATE offers SET created_at = $1 WHERE id = $2")
        .bind(staging_now - chrono::Duration::days(40))
        .bind(previous_offer)
        .execute(db.pool())
        .await
        .expect("previous supply stages");
    let _current_offer = contacted_offer(db.pool(), buyer, seller, demand, "520.00").await;
    let retention_now = chrono::Utc::now() + chrono::Duration::seconds(10);
    let retention = compute_retention(db.pool(), retention_now)
        .await
        .expect("retention computes");
    assert!(retention.previous_buyers >= 1);
    assert!(retention.recurring_buyers >= 1);
    assert!(retention.previous_sellers >= 1);
    assert!(retention.recurring_sellers >= 1);
    assert_eq!(
        retention.recurring_buyer_rate.value,
        Some(retention.recurring_buyers as f64 / retention.previous_buyers as f64)
    );
    let rendered = serde_json::to_string(&summary).expect("acquisition serializes");
    assert!(!rendered.contains("phone"));
    db.cleanup().await.expect("suite cleans up");
}

// P14-T04: professional declaration and free activity never imply paid use.
//
// Declares a free professional, records a real eligible offer, and proves
// the activity counts as free while paid and Radar indicators stay
// explicitly deferred (unavailable, never zero customers invented).
#[tokio::test]
async fn free_professional_activity_does_not_imply_paid_customer() {
    let db = TestDatabase::create("p14t04_professional")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t04_professional_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Pro Buyer", "+55 11 90000-2920").await;
    let seller = seed_active(db.pool(), "Pro Seller", "+55 11 90000-2921").await;
    declare_profile(
        db.pool(),
        seller,
        NewProfessional {
            business_name: "Corner Shop".to_owned(),
            business_type: "shop".to_owned(),
            city: "Campinas".to_owned(),
            region: "SP".to_owned(),
        },
    )
    .await
    .expect("professional declares");
    let demand = publish_demand(db.pool(), buyer, "Professional Fridge").await;
    contacted_offer(db.pool(), buyer, seller, demand, "510.00").await;

    let since = chrono::Utc::now() - chrono::Duration::days(PROFESSIONAL_ACTIVITY_DAYS);
    let activity = professional_activity(db.pool(), since)
        .await
        .expect("professional activity reads");
    assert_eq!(activity.declared, 1);
    assert_eq!(activity.free_active, 1);
    assert!(!activity.paid.available);
    assert!(!is_paid_customer(false));
    assert!(is_paid_customer(true));
    assert!(!paid_indicator().available);
    assert!(!radar_adoption().available);

    // Allowance friction assembles from observed counts without consuming
    // any successful-action quota by itself.
    let friction = allowance_friction(4, 3, 1).expect("friction assembles");
    assert_eq!(friction.eventual_success.value, Some(0.75));
    let rendered = serde_json::to_string(&activity).expect("activity serializes");
    for absent in ["phone", "lookup", "cipher", "token", "reporter", "secret"] {
        assert!(!rendered.contains(absent), "no {absent} in activity");
    }
    db.cleanup().await.expect("suite cleans up");
}

// P14-T04: reviewed incident rate differs from raw report volume.
//
// Files three grouped allegations about one contacted offer (duplicates
// group into the standing case), reviews exactly one case valid with a
// staged 26-hour decision delay, and proves the valid-incident rate uses
// reviewed validity while the raw rate keeps every allegation.
#[tokio::test]
async fn reviewed_incident_rate_differs_from_raw_volume() {
    let db = TestDatabase::create("p14t04_incidents")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t04_incidents_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Incident Buyer", "+55 11 90000-2930").await;
    let seller = seed_active(db.pool(), "Incident Seller", "+55 11 90000-2931").await;
    let demand = publish_demand(db.pool(), buyer, "Incident Fridge").await;
    let offer = contacted_offer(db.pool(), buyer, seller, demand, "500.00").await;
    let target = ReportInput {
        target_kind: "offer".to_owned(),
        target_id: offer,
        reason: "spam".to_owned(),
        detail: "repeated allegation".to_owned(),
    };
    let first = submit_grouped_report(db.pool(), buyer, target.clone())
        .await
        .expect("first allegation files");
    // Two refilings group into the standing incident; the same reporter's
    // repeats land as duplicate information without extra weight.
    for _ in 0..2 {
        submit_grouped_report(db.pool(), buyer, target.clone())
            .await
            .expect("grouped refiling files");
    }
    // Review staging disclosed: exactly one standing case becomes valid
    // 26 hours after intake; duplicates never become separate validity.
    sqlx::query(
        "UPDATE report_cases SET status = 'valid',
         updated_at = created_at + interval '26 hours' WHERE id = $1",
    )
    .bind(first.case_id)
    .execute(db.pool())
    .await
    .expect("valid review stages");

    let metrics = incident_metrics(db.pool())
        .await
        .expect("incident metrics read");
    assert_eq!(metrics.raw_reports, 3);
    assert_eq!(metrics.valid_incidents, 1);
    assert!(metrics.eligible_interactions >= 1);
    assert_ne!(metrics.valid_incident_rate, metrics.raw_report_rate);
    assert_eq!(
        metrics.valid_incident_rate.value,
        Some(metrics.valid_incidents as f64 / metrics.eligible_interactions as f64)
    );
    let delay = metrics
        .median_review_delay_hours
        .expect("review delay discloses");
    assert!(
        (delay - 26.0).abs() < 0.1,
        "median delay discloses staged 26h, got {delay}"
    );
    let rendered = serde_json::to_string(&metrics).expect("incidents serialize");
    for absent in ["phone", "reporter", "destination", "token", "secret"] {
        assert!(!rendered.contains(absent), "no {absent} in incidents");
    }
    db.cleanup().await.expect("suite cleans up");
}
