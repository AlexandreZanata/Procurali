//! Contact-initiation HTTP boundary: the author's handoff, nothing else.
//!
//! Routes: `POST /api/v1/requests/:request_id/offers/:offer_id/contact`
//! starts one guarded handoff for the authenticated buyer (201, or 200
//! replaying an established combination). Bodies are closed
//! (`deny_unknown_fields`): handoff identity, acknowledged terms, and entry
//! source exist — author, seller, phone, state, and every other key do not,
//! and are refused as unknown. The receipt carries identifiers plus the
//! current verified destination for the initiating buyer only: strangers
//! share one 404, anonymous callers one 401, and every refusal shape
//! carries no destination key at all.
//!
//! Authentication reuses current-state sessions (stale sessions answer 401)
//! with the same-host origin contract as the sibling boundaries (403
//! `forbidden_origin`). Phone keys arrive with route state and travel only
//! into the guarded operation — never logs, never responses beyond the
//! buyer's own destination. The cookie and origin helpers repeat here
//! because cross-file sharing belongs to the production merge that unifies
//! all route modules.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::errors::{ApiError, Code};
use crate::application::sessions::authenticate;
use crate::application::start_contact::{start_contact, ContactError, ContactInput};
use crate::persistence::users::PhoneKeys;

/// Shared state for the contact routes: the pool plus the phone keys the
/// guarded operation needs to reveal the authorized destination.
#[derive(Clone)]
pub struct ContactsState {
    pool: sqlx::PgPool,
    lookup_key: String,
    encryption_key: String,
}

impl ContactsState {
    /// Assemble route state from explicit parts. Keys stay in memory only.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool, lookup_key: String, encryption_key: String) -> Self {
        Self {
            pool,
            lookup_key,
            encryption_key,
        }
    }
}

/// Mount the contact routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: ContactsState) -> Router {
    Router::new()
        .route(
            "/api/v1/requests/{request_id}/offers/{offer_id}/contact",
            post(start),
        )
        .with_state(state)
}

/// Handoff body: exactly the idempotent intent contract.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContactBody {
    handoff_id: Option<String>,
    expected_offer_terms: Option<i32>,
    entry_source: Option<String>,
}

/// Buyer handoff receipt: identifiers plus the current verified destination.
/// Nothing else — no ciphertext, no other phone, no party beyond the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct HandoffResponse {
    contact_id: uuid::Uuid,
    request_id: uuid::Uuid,
    offer_id: uuid::Uuid,
    cycle_number: i32,
    destination: String,
    repeat: bool,
}

fn contact_error(error: ContactError) -> Response {
    match error {
        ContactError::MissingField(field) => {
            ApiError::new(Code::MissingField, "required contact field is missing")
                .with_field(field, Code::MissingField)
                .into_response()
        }
        ContactError::InvalidField(field) => {
            ApiError::new(Code::InvalidField, "contact field is invalid")
                .with_field(field, Code::InvalidField)
                .into_response()
        }
        ContactError::ForbiddenState => {
            ApiError::new(Code::ForbiddenState, "offer cannot take contact").into_response()
        }
        ContactError::ExpiredRequest => {
            ApiError::new(Code::Expired, "request deadline passed").into_response()
        }
        ContactError::StaleTerms => {
            ApiError::new(Code::ConflictRevision, "offer terms moved").into_response()
        }
        ContactError::SellerNotActive => {
            ApiError::new(Code::ForbiddenState, "seller cannot take contact").into_response()
        }
        // No blocked-pair code exists in the closed registry; the 403
        // capability refusal keeps the message exact.
        ContactError::Blocked => {
            ApiError::new(Code::ForbiddenRole, "relationship is blocked").into_response()
        }
        ContactError::ProhibitedCategory => {
            ApiError::new(Code::ProhibitedCategory, "category is prohibited").into_response()
        }
        ContactError::NotActive => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        ContactError::NotFound => ApiError::new(Code::NotFound, "offer not found").into_response(),
        ContactError::StorageFailed => ApiError::internal().into_response(),
    }
}

/// Extract the session token from a `Cookie` header value, if present.
fn session_token(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        if name.trim() == super::auth::SESSION_COOKIE {
            let token = value.trim().to_owned();
            if token.is_empty() {
                None
            } else {
                Some(token)
            }
        } else {
            None
        }
    })
}

/// Same-host origin for this boundary's unsafe methods: `Origin` (else
/// `Referer`) must match the request's own host (config-free subset shared
/// with the sibling boundaries; unification lands with the production
/// merge).
fn same_host_origin(headers: &HeaderMap) -> bool {
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            let referer = headers
                .get(axum::http::header::REFERER)
                .and_then(|value| value.to_str().ok())?;
            let after_scheme = referer.split_once("://")?.1;
            let host = after_scheme.split('/').next()?;
            let scheme = referer.split_once("://")?.0;
            Some(format!("{scheme}://{host}"))
        });
    match (
        origin,
        headers
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    ) {
        (Some(origin), Some(host)) => {
            origin == format!("http://{host}") || origin == format!("https://{host}")
        }
        _ => false,
    }
}

async fn authenticated_user(
    state: &ContactsState,
    headers: &HeaderMap,
) -> Result<uuid::Uuid, ApiError> {
    let token = match session_token(headers) {
        Some(token) => token,
        None => {
            return Err(ApiError::new(Code::Unauthenticated, "no usable session"));
        }
    };
    match authenticate(&state.pool, &token).await {
        Ok(Some(account)) => Ok(account.user.id),
        Ok(None) => Err(ApiError::new(Code::Unauthenticated, "no usable session")),
        Err(_) => Err(ApiError::internal()),
    }
}

/// Start one guarded handoff for the owning buyer: 201 on first contact,
/// 200 replaying an established combination.
async fn start(
    State(state): State<ContactsState>,
    headers: HeaderMap,
    Path((request_id, offer_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    body: Bytes,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if !same_host_origin(&headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "code": "forbidden_origin",
                "message": "cross-origin request refused",
            })),
        )
            .into_response();
    }
    let input: ContactBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    let keys = PhoneKeys {
        lookup_key: &state.lookup_key,
        encryption_key: &state.encryption_key,
    };
    match start_contact(
        &state.pool,
        &keys,
        user_id,
        request_id,
        offer_id,
        ContactInput {
            handoff_id: input.handoff_id,
            expected_offer_terms: input.expected_offer_terms,
            entry_source: input.entry_source,
        },
    )
    .await
    {
        Ok(handoff) => {
            let body = Json(HandoffResponse {
                contact_id: handoff.contact_id,
                request_id: handoff.request_id,
                offer_id: handoff.offer_id,
                cycle_number: handoff.cycle_number,
                destination: handoff.destination,
                repeat: handoff.repeat,
            });
            if handoff.repeat {
                (StatusCode::OK, body).into_response()
            } else {
                (StatusCode::CREATED, body).into_response()
            }
        }
        Err(error) => contact_error(error),
    }
}
