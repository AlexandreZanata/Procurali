//! Offer-submission HTTP boundary: guarded creation on eligible demand.
//!
//! Routes: `POST /api/v1/requests/:id/offers` submits one offer for the
//! authenticated seller (201). Bodies are closed (`deny_unknown_fields`):
//! cycle, revision, description, price, condition, locality, notes, and the
//! two availability declarations exist — author, state, phone, and every
//! other key do not, and are refused as unknown. The receipt carries terms
//! plus slot facts only: the buyer appears solely as the request
//! identifier, never an account, phone, or contact reference.
//!
//! Authentication reuses current-state sessions (stale sessions answer 401)
//! with the same-host origin contract as the sibling boundaries (403
//! `forbidden_origin`). The cookie name is shared from the session
//! boundary; the small parsing/origin helpers repeat here because
//! cross-file sharing belongs to the production merge that unifies all route
//! modules.

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
use crate::application::submit_offer::{submit_offer, OfferInput, SubmitError};
use crate::domain::money::MoneyError;

/// Shared state for the offer routes: the pool only.
#[derive(Clone)]
pub struct OffersState {
    pool: sqlx::PgPool,
}

impl OffersState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the offer routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: OffersState) -> Router {
    Router::new()
        .route("/api/v1/requests/{id}/offers", post(submit))
        .with_state(state)
}

/// Submission body: exactly the observed-terms contract. Observed cycle and
/// revision travel as numbers; everything else arrives as text or
/// declarations.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct OfferBody {
    revision_number: Option<i32>,
    cycle_number: Option<i32>,
    description: Option<String>,
    price: Option<String>,
    condition: Option<String>,
    city_code: Option<String>,
    region_code: Option<String>,
    notes: Option<String>,
    available: Option<bool>,
    available_in_city: Option<bool>,
}

/// Seller receipt: terms plus slot facts. No author echo and no phone
/// material by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct OfferResponse {
    id: uuid::Uuid,
    request_id: uuid::Uuid,
    cycle_number: i32,
    revision_number: i32,
    description: String,
    price: String,
    condition: String,
    city_code: String,
    region_code: String,
    notes: String,
    state: String,
}

/// Exact-grammar money refusal with the price field attached.
fn money_error(error: MoneyError) -> ApiError {
    let base = ApiError::from(error);
    let code = match error {
        MoneyError::Empty => Code::MissingField,
        MoneyError::Malformed => Code::MalformedAmount,
        MoneyError::NonPositive => Code::NonPositiveAmount,
        MoneyError::Overprecision => Code::Overprecision,
        MoneyError::Overflow => Code::AmountTooLarge,
    };
    base.with_field("price", code)
}

fn submit_error(error: SubmitError) -> Response {
    match error {
        SubmitError::MissingField(field) => {
            ApiError::new(Code::MissingField, "required offer field is missing")
                .with_field(field, Code::MissingField)
                .into_response()
        }
        SubmitError::InvalidField(field) => {
            ApiError::new(Code::InvalidField, "offer field is invalid")
                .with_field(field, Code::InvalidField)
                .into_response()
        }
        SubmitError::InvalidAmount(error) => money_error(error).into_response(),
        SubmitError::OverBudget => ApiError::new(Code::AmountTooLarge, "price exceeds the budget")
            .with_field("price", Code::AmountTooLarge)
            .into_response(),
        // No self-offer or blocked-pair codes exist in the closed registry;
        // both are 403 capability refusals with distinct messages.
        SubmitError::SelfOffer => {
            ApiError::new(Code::ForbiddenRole, "cannot offer on own request").into_response()
        }
        SubmitError::Blocked => {
            ApiError::new(Code::ForbiddenRole, "relationship is blocked").into_response()
        }
        SubmitError::NotActive => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        SubmitError::ForbiddenState => {
            ApiError::new(Code::ForbiddenState, "request cannot take offers").into_response()
        }
        SubmitError::ExpiredRequest => {
            ApiError::new(Code::Expired, "request deadline passed").into_response()
        }
        // No stale-cycle code exists; both observed-terms conflicts share
        // the revision-conflict code with distinct messages.
        SubmitError::StaleRevision => {
            ApiError::new(Code::ConflictRevision, "requirement revision moved").into_response()
        }
        SubmitError::StaleCycle => {
            ApiError::new(Code::ConflictRevision, "request cycle moved").into_response()
        }
        SubmitError::RetiredCategory => {
            ApiError::new(Code::RetiredCategory, "category is retired").into_response()
        }
        SubmitError::ProhibitedCategory => {
            ApiError::new(Code::ProhibitedCategory, "category is prohibited").into_response()
        }
        // No disabled-city registry entry exists; the refusal still names
        // the exact requirement instead of inventing a code.
        SubmitError::CityNotEnabled => ApiError::new(Code::InvalidField, "city is not enabled")
            .with_field("city_code", Code::InvalidField)
            .into_response(),
        SubmitError::NotFound => ApiError::new(Code::NotFound, "request not found").into_response(),
        SubmitError::StorageFailed => ApiError::internal().into_response(),
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
    state: &OffersState,
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

/// Submit one offer on an eligible request for the authenticated seller.
async fn submit(
    State(state): State<OffersState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
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
    let input: OfferBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    match submit_offer(
        &state.pool,
        user_id,
        id,
        OfferInput {
            revision_number: input.revision_number,
            cycle_number: input.cycle_number,
            description: input.description,
            price: input.price,
            condition: input.condition,
            city_code: input.city_code,
            region_code: input.region_code,
            notes: input.notes,
            available: input.available,
            available_in_city: input.available_in_city,
        },
    )
    .await
    {
        Ok(submitted) => (
            StatusCode::CREATED,
            Json(OfferResponse {
                id: submitted.id,
                request_id: submitted.request_id,
                cycle_number: submitted.cycle_number,
                revision_number: submitted.revision_number,
                description: submitted.description,
                price: submitted.price,
                condition: submitted.condition,
                city_code: submitted.city_code,
                region_code: submitted.region_code,
                notes: submitted.notes,
                state: submitted.state,
            }),
        )
            .into_response(),
        Err(error) => submit_error(error),
    }
}
