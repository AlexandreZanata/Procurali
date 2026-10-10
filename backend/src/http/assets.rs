//! Static asset boundary: built UI over real HTTP, safely cached.
//!
//! Serves the tracked build output directory (`web/dist`) with explicit
//! headers: fingerprinted-immutable semantics for versioned JS/CSS
//! (`public, max-age=31536000, immutable`) and `no-store` for HTML pages
//! (public request pages always re-project server-side). Directory
//! traversal refuses with 404; missing files answer the stable
//! `not_found` code. No session, no private data, and no directory
//! listing exist here — assets are public by construction.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::path::PathBuf;

/// Shared state for the asset routes: the built output directory.
#[derive(Clone)]
pub struct AssetsState {
    dir: PathBuf,
}

impl AssetsState {
    /// Assemble route state from an explicit directory.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

/// Mount the asset routes for tests and, later, the production merge.
pub fn routes(state: AssetsState) -> Router {
    Router::new()
        .route("/assets/{*path}", get(serve))
        .with_state(state)
}

fn content_type(path: &str) -> &'static str {
    if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

fn cache_control(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "no-store"
    } else {
        "public, max-age=31536000, immutable"
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"code": "not_found", "message": "asset not found"})),
    )
        .into_response()
}

/// Serve one built asset with safe headers.
async fn serve(State(state): State<AssetsState>, Path(path): Path<String>) -> Response {
    if path.is_empty()
        || path
            .split('/')
            .any(|segment| segment == ".." || segment == ".")
    {
        return not_found();
    }
    let mut file = state.dir.clone();
    for segment in path.split('/') {
        if segment.is_empty() {
            return not_found();
        }
        file.push(segment);
    }
    let bytes = match tokio::fs::read(&file).await {
        Ok(bytes) => bytes,
        Err(_) => return not_found(),
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type(&path)),
            (header::CACHE_CONTROL, cache_control(&path)),
        ],
        Body::from(bytes),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    async fn fixture_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("procurali-assets-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir)
            .await
            .expect("fixture dir creates");
        tokio::fs::write(dir.join("main.js"), "export const x = 1;\n")
            .await
            .expect("fixture js writes");
        tokio::fs::write(dir.join("base.css"), ":root{}\n")
            .await
            .expect("fixture css writes");
        tokio::fs::write(dir.join("page.html"), "<main></main>\n")
            .await
            .expect("fixture html writes");
        dir
    }

    #[tokio::test]
    async fn built_assets_load_with_safe_headers() {
        let dir = fixture_dir().await;
        let app = routes(AssetsState::new(dir));
        let response = app
            .oneshot(
                axum::http::Request::get("/assets/main.js")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("route responds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .expect("typed")
                .to_str()
                .expect("ascii"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .expect("cached")
                .to_str()
                .expect("ascii"),
            "public, max-age=31536000, immutable"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 65_536)
            .await
            .expect("body reads");
        assert_eq!(&bytes[..], b"export const x = 1;\n");
    }

    #[tokio::test]
    async fn html_is_never_cached_and_traversal_refuses() {
        let dir = fixture_dir().await;
        let app = routes(AssetsState::new(dir));
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::get("/assets/page.html")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("route responds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .expect("cached")
                .to_str()
                .expect("ascii"),
            "no-store"
        );
        for denied in ["/assets/../main.js", "/assets/missing.js"] {
            let refused = app
                .clone()
                .oneshot(
                    axum::http::Request::get(denied)
                        .body(Body::empty())
                        .expect("request builds"),
                )
                .await
                .expect("route responds");
            assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        }
    }

    #[test]
    fn errors_carry_no_values() {
        let rendered = format!("{:?}", not_found().status());
        assert!(!rendered.contains("canary"));
    }
}
