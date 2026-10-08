//! Graceful server lifecycle.
//!
//! The server drains in-flight requests on shutdown and never reports readiness
//! before dependencies are usable (readiness is a separate probe over
//! [`DependencyStatus`](super::http::health::DependencyStatus)).

use axum::Router;
use tokio::{net::TcpListener, sync::oneshot};

/// Serve `app` until `shutdown` resolves, then drain and return.
///
/// The sender side lives with the binary entry point (wired in a later card);
/// tests hold it directly to prove clean exit.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: oneshot::Receiver<()>,
) -> std::io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.await;
        })
        .await
}
