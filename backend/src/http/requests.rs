//! Owner request HTTP boundary: drafts, publication, lists, and details.
//!
//! Routes: `POST /api/v1/requests/drafts` saves a private draft (201);
//! `GET /api/v1/requests/drafts/:id` returns the owner's draft (200);
//! `PUT /api/v1/requests/drafts/:id` replaces its requirements (200);
//! `POST /api/v1/requests/drafts/:id/publication` publishes it into its
//! first seven-day cycle (201, or 200 replaying an existing publication);
//! `GET /api/v1/requests` lists the owner's requests newest-first with
//! derived timing and permitted actions (200);
//! `GET /api/v1/requests/:id` returns one owned request with its cycles and
//! revisions (200).
//! Missing, non-owned, and advanced rows share one 404, and anonymous
//! callers share one 401: strangers learn nothing. Draft bodies are closed
//! (`deny_unknown_fields`): author, state, cycle, timestamp, phone, or any
//! other overposted key is a 400. Responses carry requirements, lifecycle,
//! timing, and permitted actions only — no author echo, no phone material.
//!
//! Authentication reuses current-state sessions (stale sessions answer 401);
//! unsafe methods share the same-host origin contract as the account boundary
//! (403 `forbidden_origin`). The cookie name is shared from the session
//! boundary; the small parsing/origin helpers repeat here because
//! cross-file sharing belongs to the production merge that unifies all route
//! modules.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::errors::{ApiError, Code};
use crate::application::publish_request::{publish_request, PublishError, PublishedRequest};
use crate::application::request_drafts::{
    create_draft, edit_draft, get_draft, DraftError, DraftInput,
};
use crate::application::request_reads::{get_request, list_requests, ReadError};
use crate::application::sessions::authenticate;
use crate::domain::money::MoneyError;

/// Shared state for the draft routes: the pool only. No provider, keys, or
/// allowance configuration exists on this boundary.
#[derive(Clone)]
pub struct RequestsState {
    pool: sqlx::PgPool,
}

impl RequestsState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the request routes under `/api/v1` for tests and, later, the
/// production router merge. The static `/drafts` segments take precedence
/// over the `{id}` captures at the same position.
pub fn routes(state: RequestsState) -> Router {
    Router::new()
        .route("/api/v1/requests/drafts", post(create))
        .route("/api/v1/requests/drafts/{id}", get(read))
        .route("/api/v1/requests/drafts/{id}", put(edit))
        .route("/api/v1/requests/drafts/{id}/publication", post(publish))
        .route("/api/v1/requests", get(list))
        .route("/api/v1/requests/{id}", get(detail))
        .with_state(state)
}

/// Draft body: exactly the requirement keys. Author, state, cycle,
/// timestamp, contact, and every other key do not exist here and are refused
/// as unknown.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftBody {
    title: Option<String>,
    category_code: Option<String>,
    budget: Option<String>,
    condition: Option<String>,
    city_code: Option<String>,
    region_code: Option<String>,
    notes: Option<String>,
}

/// Owner draft receipt: requirements plus lifecycle state. No author echo and
/// no phone material by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DraftResponse {
    id: uuid::Uuid,
    title: String,
    category_code: String,
    budget: String,
    condition: String,
    city_code: String,
    region_code: String,
    notes: String,
    state: String,
}

fn receipt(draft: crate::application::request_drafts::Draft) -> DraftResponse {
    DraftResponse {
        id: draft.id,
        title: draft.title,
        category_code: draft.category_code,
        budget: draft.budget,
        condition: draft.condition,
        city_code: draft.city_code,
        region_code: draft.region_code,
        notes: draft.notes,
        state: draft.state,
    }
}

/// Owner request receipt: requirements, derived timing, and the permitted
/// actions for the current account, category, and time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SummaryResponse {
    id: uuid::Uuid,
    title: String,
    category_code: String,
    budget: String,
    condition: String,
    city_code: String,
    region_code: String,
    notes: String,
    state: String,
    cycle_number: i32,
    revision_number: i32,
    original_published_at: Option<chrono::DateTime<chrono::Utc>>,
    deadline: Option<chrono::DateTime<chrono::Utc>>,
    expired: bool,
    actions: Vec<&'static str>,
}

/// One activation cycle for owner history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CycleResponse {
    cycle_number: i32,
    started_at: chrono::DateTime<chrono::Utc>,
    deadline: chrono::DateTime<chrono::Utc>,
}

/// One requirement revision for owner history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RevisionResponse {
    revision_number: i32,
    title: String,
    category_code: String,
    budget: String,
    condition: String,
    city_code: String,
    region_code: String,
    notes: String,
}

/// Owner detail: the summary flattened with its cycles and revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DetailResponse {
    #[serde(flatten)]
    summary: SummaryResponse,
    cycles: Vec<CycleResponse>,
    revisions: Vec<RevisionResponse>,
}

/// Bounded owner list with the total owned count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ListResponse {
    requests: Vec<SummaryResponse>,
    total: i64,
}

fn summary_response(summary: crate::application::request_reads::RequestSummary) -> SummaryResponse {
    SummaryResponse {
        id: summary.id,
        title: summary.title,
        category_code: summary.category_code,
        budget: summary.budget,
        condition: summary.condition,
        city_code: summary.city_code,
        region_code: summary.region_code,
        notes: summary.notes,
        state: summary.state,
        cycle_number: summary.cycle_number,
        revision_number: summary.revision_number,
        original_published_at: summary.original_published_at,
        deadline: summary.deadline,
        expired: summary.expired,
        actions: summary
            .actions
            .iter()
            .map(|action| action.as_str())
            .collect(),
    }
}

fn read_error(error: ReadError) -> Response {
    match error {
        ReadError::NotActive => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        ReadError::NotFound => ApiError::new(Code::NotFound, "request not found").into_response(),
        ReadError::InvalidPage => {
            ApiError::new(Code::InvalidField, "pagination is invalid").into_response()
        }
        ReadError::StorageFailed => ApiError::internal().into_response(),
    }
}

fn publish_error(error: PublishError) -> Response {
    match error {
        PublishError::InvalidField(field) => {
            ApiError::new(Code::InvalidField, "publication field is invalid")
                .with_field(field, Code::InvalidField)
                .into_response()
        }
        PublishError::InvalidAmount(error) => money_error(error).into_response(),
        PublishError::UnknownCategory => {
            ApiError::new(Code::UnknownCategory, "unknown category code").into_response()
        }
        PublishError::UnknownCity => {
            ApiError::new(Code::UnknownCity, "unknown city code").into_response()
        }
        // No `unknown_region` or `disabled_city` registry entries exist; the
        // refusals still name the exact requirement instead of inventing
        // codes.
        PublishError::UnknownRegion => ApiError::new(Code::InvalidField, "unknown region code")
            .with_field("region_code", Code::InvalidField)
            .into_response(),
        PublishError::RetiredCategory => {
            ApiError::new(Code::RetiredCategory, "category is retired").into_response()
        }
        PublishError::ProhibitedCategory => {
            ApiError::new(Code::ProhibitedCategory, "category is prohibited").into_response()
        }
        PublishError::CityNotEnabled => ApiError::new(Code::InvalidField, "city is not enabled")
            .with_field("city_code", Code::InvalidField)
            .into_response(),
        PublishError::NotActive => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        PublishError::NotFound => {
            ApiError::new(Code::NotFound, "request not found").into_response()
        }
        PublishError::ForbiddenState => {
            ApiError::new(Code::ForbiddenState, "request cannot be published").into_response()
        }
        PublishError::StorageFailed => ApiError::internal().into_response(),
    }
}

/// Exact-grammar money refusal with the budget field attached.
fn money_error(error: MoneyError) -> ApiError {
    let base = ApiError::from(error);
    let code = match error {
        MoneyError::Empty => Code::MissingField,
        MoneyError::Malformed => Code::MalformedAmount,
        MoneyError::NonPositive => Code::NonPositiveAmount,
        MoneyError::Overprecision => Code::Overprecision,
        MoneyError::Overflow => Code::AmountTooLarge,
    };
    base.with_field("budget", code)
}

fn draft_error(error: DraftError) -> Response {
    match error {
        DraftError::MissingField(field) => {
            ApiError::new(Code::MissingField, "required draft field is missing")
                .with_field(field, Code::MissingField)
                .into_response()
        }
        DraftError::InvalidField(field) => {
            ApiError::new(Code::InvalidField, "draft field is invalid")
                .with_field(field, Code::InvalidField)
                .into_response()
        }
        DraftError::InvalidAmount(error) => money_error(error).into_response(),
        DraftError::UnknownCategory => {
            ApiError::new(Code::UnknownCategory, "unknown category code").into_response()
        }
        DraftError::UnknownCity => {
            ApiError::new(Code::UnknownCity, "unknown city code").into_response()
        }
        // No `unknown_region` registry entry exists; the refusal still names
        // the exact requirement instead of inventing a code.
        DraftError::UnknownRegion => ApiError::new(Code::InvalidField, "unknown region code")
            .with_field("region_code", Code::InvalidField)
            .into_response(),
        DraftError::NotActive => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        DraftError::NotFound => ApiError::new(Code::NotFound, "request not found").into_response(),
        DraftError::StorageFailed => ApiError::internal().into_response(),
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
/// with the account boundary; unification lands with the production merge).
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
    state: &RequestsState,
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

fn origin_refused(headers: &HeaderMap) -> Option<Response> {
    if same_host_origin(headers) {
        None
    } else {
        Some(
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "code": "forbidden_origin",
                    "message": "cross-origin request refused",
                })),
            )
                .into_response(),
        )
    }
}

fn draft_input(body: DraftBody) -> DraftInput {
    DraftInput {
        title: body.title,
        category_code: body.category_code,
        budget: body.budget,
        condition: body.condition,
        city_code: body.city_code,
        region_code: body.region_code,
        notes: body.notes,
    }
}

/// Save one private draft for the authenticated owner.
async fn create(State(state): State<RequestsState>, headers: HeaderMap, body: Bytes) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if let Some(response) = origin_refused(&headers) {
        return response;
    }
    let input: DraftBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    match create_draft(&state.pool, user_id, draft_input(input)).await {
        Ok(draft) => (StatusCode::CREATED, Json(receipt(draft))).into_response(),
        Err(error) => draft_error(error),
    }
}

/// Return the owner's draft, if it is still a draft.
async fn read(
    State(state): State<RequestsState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    match get_draft(&state.pool, user_id, id).await {
        Ok(draft) => (StatusCode::OK, Json(receipt(draft))).into_response(),
        Err(error) => draft_error(error),
    }
}

/// Replace the owner's draft requirements while it is still a draft.
async fn edit(
    State(state): State<RequestsState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
    body: Bytes,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if let Some(response) = origin_refused(&headers) {
        return response;
    }
    let input: DraftBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    match edit_draft(&state.pool, user_id, id, draft_input(input)).await {
        Ok(draft) => (StatusCode::OK, Json(receipt(draft))).into_response(),
        Err(error) => draft_error(error),
    }
}

/// Publish the owner's draft into its first seven-day cycle: 201 on first
/// publication, 200 replaying an existing one. The status is decided from a
/// best-effort pre-read; the receipt is always the current publication truth.
async fn publish(
    State(state): State<RequestsState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if let Some(response) = origin_refused(&headers) {
        return response;
    }
    let replay = matches!(
        crate::persistence::requests::request(&state.pool, id).await,
        Ok(Some(stored)) if stored.state == "active" && stored.author_id == user_id
    );
    match publish_request(&state.pool, user_id, id).await {
        Ok(published) => published_response(published, replay),
        Err(error) => publish_error(error),
    }
}

fn published_response(published: PublishedRequest, replay: bool) -> Response {
    let body = Json(serde_json::json!({
        "id": published.id,
        "title": published.title,
        "category_code": published.category_code,
        "budget": published.budget,
        "condition": published.condition,
        "city_code": published.city_code,
        "region_code": published.region_code,
        "notes": published.notes,
        "state": published.state,
        "cycle_number": published.cycle_number,
        "revision_number": published.revision_number,
        "original_published_at": published.original_published_at,
        "deadline": published.deadline,
    }));
    if replay {
        (StatusCode::OK, body).into_response()
    } else {
        (StatusCode::CREATED, body).into_response()
    }
}

/// Parse one optional pagination value: absent means unset, present must be
/// decimal digits. Parser failures are 400s with a stable shape, never the
/// framework's plain-text rejection.
fn pagination(params: &HashMap<String, String>, key: &str) -> Result<Option<u32>, ApiError> {
    match params.get(key) {
        None => Ok(None),
        Some(raw) => raw
            .parse::<u32>()
            .map(Some)
            .map_err(|_| ApiError::new(Code::InvalidField, "pagination is invalid")),
    }
}

/// List the owner's requests newest-first with derived actions.
async fn list(
    State(state): State<RequestsState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    let limit = match pagination(&params, "limit") {
        Ok(limit) => limit,
        Err(error) => return error.into_response(),
    };
    let offset = match pagination(&params, "offset") {
        Ok(offset) => offset,
        Err(error) => return error.into_response(),
    };
    // Unknown query keys are ignored: pagination names the only contract.
    match list_requests(&state.pool, user_id, limit, offset).await {
        Ok((summaries, total)) => (
            StatusCode::OK,
            Json(ListResponse {
                requests: summaries.into_iter().map(summary_response).collect(),
                total,
            }),
        )
            .into_response(),
        Err(error) => read_error(error),
    }
}

/// Return one owned request with its cycles and revisions.
async fn detail(
    State(state): State<RequestsState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    match get_request(&state.pool, user_id, id).await {
        Ok(detail) => (
            StatusCode::OK,
            Json(DetailResponse {
                summary: summary_response(detail.summary),
                cycles: detail
                    .cycles
                    .into_iter()
                    .map(|cycle| CycleResponse {
                        cycle_number: cycle.cycle_number,
                        started_at: cycle.started_at,
                        deadline: cycle.deadline,
                    })
                    .collect(),
                revisions: detail
                    .revisions
                    .into_iter()
                    .map(|revision| RevisionResponse {
                        revision_number: revision.revision_number,
                        title: revision.title,
                        category_code: revision.category_code,
                        budget: revision.budget,
                        condition: revision.condition,
                        city_code: revision.city_code,
                        region_code: revision.region_code,
                        notes: revision.notes,
                    })
                    .collect(),
            }),
        )
            .into_response(),
        Err(error) => read_error(error),
    }
}
