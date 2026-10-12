//! Privacy headers boundary: safe defaults on every response.
//!
//! Applies `Cache-Control: no-store` (private pages and API payloads
//! are never stored by shared caches), `X-Content-Type-Options:
//! nosniff`, and `Referrer-Policy: same-origin` to all responses
//! passing through the layer. HSTS stays out: it must only ride HTTPS
//! responses, which the terminating proxy owns (see infra/Caddyfile).
//! Cookie flags (Secure/HttpOnly/SameSite) stay with the session
//! boundary that mints the cookie.

use axum::{
    body::Body,
    http::{header, Request, Response},
};
use tower::{Layer, ServiceExt};

/// Layer applying the privacy header set. No configuration, no secrets.
#[derive(Clone, Copy, Debug, Default)]
pub struct SecurityHeadersLayer;

impl<S> Layer<S> for SecurityHeadersLayer {
    type Service = SecurityHeadersService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SecurityHeadersService { inner }
    }
}

/// Service inserting the header set on every response.
#[derive(Clone, Copy, Debug)]
pub struct SecurityHeadersService<S> {
    inner: S,
}

impl<S> tower::Service<Request<Body>> for SecurityHeadersService<S>
where
    S: tower::Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Error: Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Response<Body>, S::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let inner = self.inner.clone();
        Box::pin(async move {
            let mut response = inner.oneshot(request).await?;
            let headers = response.headers_mut();
            headers.insert(
                header::CACHE_CONTROL,
                "no-store".parse().expect("static header parses"),
            );
            headers.insert(
                "x-content-type-options",
                "nosniff".parse().expect("static header parses"),
            );
            headers.insert(
                "referrer-policy",
                "same-origin".parse().expect("static header parses"),
            );
            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use tower::ServiceExt;

    async fn through_layer() -> HeaderMap {
        let inner = tower::service_fn(|_request: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(
                Response::builder()
                    .status(200)
                    .body(Body::from("ok"))
                    .expect("fixture response builds"),
            )
        });
        let service = SecurityHeadersLayer.layer(inner);
        let response = service
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("layer responds");
        assert_eq!(response.status(), 200);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 65_536)
                .await
                .expect("body reads"),
            "ok"
        );
        // Re-run for header assertions on a fresh response.
        let service =
            SecurityHeadersLayer.layer(tower::service_fn(|_request: Request<Body>| async {
                Ok::<_, std::convert::Infallible>(
                    Response::builder()
                        .status(200)
                        .body(Body::empty())
                        .expect("fixture response builds"),
                )
            }));
        service
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("layer responds")
            .headers()
            .clone()
    }

    #[tokio::test]
    async fn private_responses_carry_no_store_and_hardening() {
        let headers = through_layer().await;
        assert_eq!(
            headers
                .get(header::CACHE_CONTROL)
                .expect("cached")
                .to_str()
                .expect("ascii"),
            "no-store"
        );
        assert_eq!(
            headers
                .get("x-content-type-options")
                .expect("typed")
                .to_str()
                .expect("ascii"),
            "nosniff"
        );
        assert_eq!(
            headers
                .get("referrer-policy")
                .expect("policy")
                .to_str()
                .expect("ascii"),
            "same-origin"
        );
        assert!(headers.get(header::STRICT_TRANSPORT_SECURITY).is_none());
    }

    #[test]
    fn errors_carry_no_values() {
        let rendered = format!("{:?}", SecurityHeadersLayer);
        assert!(!rendered.contains("canary"));
    }
}
