//! Catalog acceptance (P05-T01): categories, cities, regions, public routes.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL 18.6:
//! - excluded item classes cannot be activated by arbitrary client labels;
//! - same-named regions in different cities remain distinct;
//! - catalog changes preserve stable historical identity;
//! - the public routes serve minimal fields with no coordinates or accounts.
//!
//! All city and region fixtures below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::http::catalogs::{routes, CatalogsState};
use procurali_backend::persistence::catalogs::{
    categories, category, regions_for_city, relabel_category, set_category_status, upsert_city,
    upsert_region, usable_for_new_cycle, CategoryStatus,
};
use serde_json::Value;
use tower::ServiceExt;

async fn seed_cities(pool: &sqlx::PgPool) {
    let mut tx = pool.begin().await.expect("transaction begins");
    upsert_city(&mut tx, "campinas", "Campinas", true)
        .await
        .expect("fixture city stores");
    upsert_city(&mut tx, "valinhos", "Valinhos", true)
        .await
        .expect("fixture city stores");
    upsert_city(&mut tx, "closed_city", "Closed City", false)
        .await
        .expect("disabled city stores");
    upsert_region(&mut tx, "campinas", "centro", "Centro")
        .await
        .expect("fixture region stores");
    upsert_region(&mut tx, "valinhos", "centro", "Centro")
        .await
        .expect("same-named region stores");
    tx.commit().await.expect("fixtures commit");
}

#[tokio::test]
async fn excluded_classes_cannot_be_activated_by_client_labels() {
    let db = TestDatabase::create("p05t01_excluded")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t01_excluded_"),
        "known suite identity in the database name"
    );
    // Exactly the six seeded categories exist; excluded classes have no rows.
    let all = categories(db.pool()).await.expect("catalog reads");
    assert_eq!(all.len(), 6);
    for code in [
        "real_estate",
        "motor_vehicles",
        "jobs",
        "services",
        "food",
        "medication",
        "animals",
        "electronics2",
    ] {
        assert!(
            category(db.pool(), code)
                .await
                .expect("lookup queries")
                .is_none(),
            "no row exists for {code}"
        );
    }
    // Lifecycle gates new use: retired and prohibited refuse, allowed passes.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(
        set_category_status(&mut tx, "tools", CategoryStatus::Retired)
            .await
            .expect("status transitions")
    );
    assert!(
        set_category_status(&mut tx, "electronics", CategoryStatus::Prohibited)
            .await
            .expect("status transitions")
    );
    assert!(
        !set_category_status(&mut tx, "motor_vehicles", CategoryStatus::Allowed)
            .await
            .expect("transition queries"),
        "unknown codes transition nothing"
    );
    tx.commit().await.expect("transitions commit");
    assert!(usable_for_new_cycle(
        &category(db.pool(), "furniture")
            .await
            .expect("lookup queries")
            .expect("seeded row reads")
    ));
    for code in ["tools", "electronics"] {
        assert!(
            !usable_for_new_cycle(
                &category(db.pool(), code)
                    .await
                    .expect("lookup queries")
                    .expect("seeded row reads")
            ),
            "{code} refuses new use"
        );
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn same_named_regions_in_different_cities_stay_distinct() {
    let db = TestDatabase::create("p05t01_regions")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t01_regions_"),
        "known suite identity in the database name"
    );
    seed_cities(db.pool()).await;
    let campinas = regions_for_city(db.pool(), "campinas")
        .await
        .expect("regions read");
    let valinhos = regions_for_city(db.pool(), "valinhos")
        .await
        .expect("regions read");
    assert_eq!(campinas.len(), 1);
    assert_eq!(valinhos.len(), 1);
    assert_eq!(campinas[0].city_code, "campinas");
    assert_eq!(valinhos[0].city_code, "valinhos");
    assert_ne!(
        (campinas[0].city_code.clone(), campinas[0].code.clone()),
        (valinhos[0].city_code.clone(), valinhos[0].code.clone()),
        "composite identities never collide"
    );
    assert!(
        procurali_backend::persistence::catalogs::region(db.pool(), "campinas", "norte")
            .await
            .expect("lookup queries")
            .is_none()
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn catalog_changes_preserve_stable_identity() {
    let db = TestDatabase::create("p05t01_identity")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t01_identity_"),
        "known suite identity in the database name"
    );
    seed_cities(db.pool()).await;
    // Relabels and status transitions keep codes; regions stay linked.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(relabel_category(
        &mut tx,
        "bicycles",
        "Bikes",
        "Bikes and non-motorized cycling goods."
    )
    .await
    .expect("relabel applies"));
    assert!(
        set_category_status(&mut tx, "bicycles", CategoryStatus::Retired)
            .await
            .expect("status transitions")
    );
    upsert_region(&mut tx, "campinas", "centro", "Downtown Center")
        .await
        .expect("region relabels");
    tx.commit().await.expect("changes commit");
    let renamed = category(db.pool(), "bicycles")
        .await
        .expect("lookup queries")
        .expect("renamed row reads");
    assert_eq!(renamed.code, "bicycles", "stable code survives relabels");
    assert_eq!(renamed.label, "Bikes");
    assert_eq!(renamed.status, CategoryStatus::Retired);
    let region = procurali_backend::persistence::catalogs::region(db.pool(), "campinas", "centro")
        .await
        .expect("lookup queries")
        .expect("region reads");
    assert_eq!(region.city_code, "campinas");
    assert_eq!(region.name, "Downtown Center");
    assert!(
        !usable_for_new_cycle(&renamed),
        "retired rows refuse new use after relabel"
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn public_catalog_routes_serve_minimal_fields() {
    let db = TestDatabase::create("p05t01_routes")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t01_routes_"),
        "known suite identity in the database name"
    );
    seed_cities(db.pool()).await;
    let app = routes(CatalogsState::new(db.pool().clone()));

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/catalogs/cities")
                .body(Body::empty())
                .expect("test request builds"),
        )
        .await
        .expect("cities respond");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    let rendered: Value = serde_json::from_slice(&bytes).expect("JSON parses");
    let cities = rendered["cities"].as_array().expect("city list");
    assert_eq!(cities.len(), 2, "only explicitly enabled cities serve");
    assert_eq!(cities[0]["code"], "campinas");
    assert_eq!(cities[0]["regions"][0]["code"], "centro");
    assert_eq!(
        cities[0].as_object().expect("object").keys().count(),
        3,
        "city shape carries exactly code, name, regions"
    );

    let response = app
        .oneshot(
            Request::get("/api/v1/catalogs/categories")
                .body(Body::empty())
                .expect("test request builds"),
        )
        .await
        .expect("categories respond");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    let rendered: Value = serde_json::from_slice(&bytes).expect("JSON parses");
    let categories = rendered["categories"].as_array().expect("category list");
    assert_eq!(categories.len(), 6);
    let codes: Vec<&str> = categories
        .iter()
        .map(|entry| entry["code"].as_str().expect("code present"))
        .collect();
    for expected in [
        "baby_kids",
        "bicycles",
        "electronics",
        "furniture",
        "home_appliances",
        "tools",
    ] {
        assert!(codes.contains(&expected), "seeded {expected} serves");
    }
    let rendered = rendered.to_string();
    for absent in ["latitude", "longitude", "coordinates", "account", "phone"] {
        assert!(!rendered.contains(absent), "no {absent} in catalog output");
    }
    // Unfiltered reads still see explicitly disabled cities (operations data,
    // never served publicly).
    assert_eq!(
        procurali_backend::persistence::catalogs::cities(db.pool(), false)
            .await
            .expect("reads")
            .len(),
        3
    );
    db.cleanup().await.expect("suite cleans up");
}
