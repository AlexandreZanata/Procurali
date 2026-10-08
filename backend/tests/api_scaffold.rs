//! HTTP scaffold acceptance: exact health schemas, dependency-honest readiness,
//! bounded requests, stable unknown-route errors, and clean shutdown.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use procurali_backend::http::health::DependencyStatus;
use procurali_backend::http::router::{build_router, MAX_BODY_BYTES};
use procurali_backend::operations::lifecycle::serve;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};
use tower::ServiceExt;

#[tokio::test]
async fn liveness_returns_its_exact_schema() {
    let app = build_router(DependencyStatus::default());
    let response = app
        .oneshot(
            Request::get("/health/live")
                .body(Body::empty())
                .expect("test request builds"),
        )
        .await
        .expect("liveness responds");
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("body reads");
    assert_eq!(
        body.as_ref(),
        br#"{"service":"procurali-backend","status":"alive"}"#
    );
}

#[tokio::test]
async fn readiness_reports_unavailable_dependency_without_values() {
    let status = DependencyStatus::default();
    let app = build_router(status.clone());
    let response = app
        .oneshot(
            Request::get("/health/ready")
                .body(Body::empty())
                .expect("test request builds"),
        )
        .await
        .expect("readiness responds");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("body reads");
    // Exact match: names only, no configuration value can hide in the payload.
    assert_eq!(
        body.as_ref(),
        br#"{"status":"not_ready","unavailable":["database"]}"#
    );

    status.set_database_ready(true);
    let app = build_router(status);
    let response = app
        .oneshot(
            Request::get("/health/ready")
                .body(Body::empty())
                .expect("test request builds"),
        )
        .await
        .expect("readiness responds");
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("body reads");
    assert_eq!(body.as_ref(), br#"{"status":"ready","unavailable":[]}"#);
}

#[tokio::test]
async fn oversized_body_and_unknown_path_have_defined_responses() {
    // Mirror of the production stack (see http::router): routes are registered
    // before layers, and the skeleton has no body-reading production route, so
    // this proof composes the same stack around a body-reading test handler.
    // Unknown routes intentionally stay cheap: 404 without buffering.
    let app = axum::Router::new()
        .route(
            "/test-echo",
            axum::routing::post(|body: axum::body::Bytes| async move { body.len().to_string() }),
        )
        .layer(
            tower::ServiceBuilder::new()
                .layer(tower_http::limit::RequestBodyLimitLayer::new(
                    MAX_BODY_BYTES,
                ))
                .layer(axum::error_handling::HandleErrorLayer::new(
                    |_: tower::BoxError| async { StatusCode::REQUEST_TIMEOUT },
                ))
                .layer(tower::timeout::TimeoutLayer::new(Duration::from_secs(30))),
        );
    let big = vec![0_u8; MAX_BODY_BYTES + MAX_BODY_BYTES / 2];
    let response = app
        .oneshot(
            Request::post("/test-echo")
                .body(Body::from(big))
                .expect("test request builds"),
        )
        .await
        .expect("limit layer responds");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let app = build_router(DependencyStatus::default());
    let response = app
        .oneshot(
            Request::post("/no-such-route")
                .body(Body::from("small".to_owned()))
                .expect("test request builds"),
        )
        .await
        .expect("fallback responds");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("body reads");
    assert_eq!(
        body.as_ref(),
        br#"{"code":"not_found","message":"unknown route"}"#
    );
}

#[tokio::test]
async fn request_time_bound_is_enforced() {
    // Mirror of the production stack with a short timeout around a slow
    // handler; see the note above for why the proof composes test-locally.
    let app = axum::Router::new()
        .route(
            "/test-slow",
            axum::routing::get(|| async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                "too late"
            }),
        )
        .layer(
            tower::ServiceBuilder::new()
                .layer(tower_http::limit::RequestBodyLimitLayer::new(
                    MAX_BODY_BYTES,
                ))
                .layer(axum::error_handling::HandleErrorLayer::new(
                    |_: tower::BoxError| async { StatusCode::REQUEST_TIMEOUT },
                ))
                .layer(tower::timeout::TimeoutLayer::new(Duration::from_millis(50))),
        );
    let response = app
        .oneshot(
            Request::get("/test-slow")
                .body(Body::empty())
                .expect("test request builds"),
        )
        .await
        .expect("timeout layer responds");
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
}

#[tokio::test]
async fn shutdown_drains_and_exits_cleanly() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ephemeral port binds");
    let address = listener.local_addr().expect("listener has an address");
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(serve(
        listener,
        build_router(DependencyStatus::default()),
        shutdown_rx,
    ));

    let mut stream = TcpStream::connect(address)
        .await
        .expect("server accepts connections");
    stream
        .write_all(b"GET /health/live HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("request writes");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("response reads");
    assert!(
        raw.starts_with(b"HTTP/1.1 200"),
        "liveness answers over real TCP"
    );

    drop(shutdown_tx);
    timeout(Duration::from_secs(5), server)
        .await
        .expect("shutdown completes promptly")
        .expect("server task joins")
        .expect("graceful exit without I/O error");

    assert!(
        TcpStream::connect(address).await.is_err(),
        "listener is closed after shutdown"
    );
}
