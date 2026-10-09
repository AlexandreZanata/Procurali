//! Public HTML boundary: safe server-rendered request pages, no framework.
//!
//! Routes: `GET /api/v1/public/requests/{id}/html` renders the request
//! template below as `text/html`. Values interpolate escaped for both
//! element and attribute contexts; user text is never raw HTML, and no
//! private offer link exists anywhere (the offer action is a static line
//! with no URL, since no frontend routes exist to point at). Terminal
//! demand renders standing without interaction; missing, draft, private,
//! hidden, suspended, prohibited, or blocked rows render one generic
//! page. Responses carry `Cache-Control: no-store` so no intermediary
//! treats a snapshot as current business authority — external previews
//! are outside this product's control and equally non-authoritative.
//!
//! Authentication is deliberately optional, exactly like the JSON public
//! boundary: usable sessions resolve their viewer for the block check,
//! while missing or stale sessions read as anonymous, never 401. The
//! cookie name is shared from the session boundary; the small parsing
//! helper repeats here because cross-file sharing belongs to the
//! production merge that unifies all route modules.

use axum::{
    extract::{Path, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE},
        HeaderMap, StatusCode,
    },
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::application::public_request::{project_request, PublicProjection};
use crate::application::sessions::authenticate;
use crate::persistence::requests::format_budget;

/// Request page template: static structure with escaped slots only.
const TEMPLATE: &str = include_str!("../../../web/public/request-template.html");

/// Shared state for the public HTML routes: the pool only.
#[derive(Clone)]
pub struct PublicHtmlState {
    pool: sqlx::PgPool,
}

impl PublicHtmlState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the public HTML routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: PublicHtmlState) -> Router {
    Router::new()
        .route("/api/v1/public/requests/{id}/html", get(page))
        .with_state(state)
}

/// Escape one value for HTML element and double-quoted attribute contexts.
fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for char in value.chars() {
        match char {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#x27;"),
            _ => escaped.push(char),
        }
    }
    escaped
}

/// Fill every template slot: full details, limited standing, or the
/// generic unavailable page. No slot is ever left unreplaced.
fn render_full(
    title: &str,
    category_code: &str,
    budget_cents: i64,
    condition: &str,
    city_code: &str,
    region_code: &str,
    author_name: &str,
) -> String {
    let title = escape_html(title);
    let detail = escape_html(&format!(
        "Budget {} · Condition {} · Category {} · Region {}, {} · Listed by {}",
        format_budget(budget_cents),
        condition,
        category_code,
        region_code,
        city_code,
        author_name,
    ));
    TEMPLATE
        .replace(
            "{{PAGE_TITLE}}",
            &escape_html(&format!("{title} — Procurali")),
        )
        .replace("{{META_TITLE}}", &title)
        .replace("{{META_DESCRIPTION}}", &detail)
        .replace("{{HEADING}}", &title)
        .replace("{{DETAIL_LINE}}", &detail)
        .replace(
            "{{ACTION_LINE}}",
            "Sellers with this item can make an offer in the app.",
        )
}

/// Fill the template for terminal standing: status only, no interaction.
/// The wording names no offer link and no private surface.
fn render_limited(status: &str) -> String {
    let heading = escape_html(&format!("Request {status}"));
    let detail = escape_html(&format!("This request is {status}."));
    TEMPLATE
        .replace(
            "{{PAGE_TITLE}}",
            &escape_html(&format!("{heading} — Procurali")),
        )
        .replace("{{META_TITLE}}", &heading)
        .replace("{{META_DESCRIPTION}}", &detail)
        .replace("{{HEADING}}", &heading)
        .replace("{{DETAIL_LINE}}", &detail)
        .replace("{{ACTION_LINE}}", "")
}

/// Fill the template for everything unavailable: static metadata only, no
/// request content of any kind.
fn render_unavailable() -> String {
    TEMPLATE
        .replace("{{PAGE_TITLE}}", "Unavailable request — Procurali")
        .replace("{{META_TITLE}}", "Unavailable request")
        .replace("{{META_DESCRIPTION}}", "This request is unavailable.")
        .replace("{{HEADING}}", "Unavailable request")
        .replace("{{DETAIL_LINE}}", "This request is unavailable.")
        .replace("{{ACTION_LINE}}", "")
}

fn html_response(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (CONTENT_TYPE, "text/html; charset=utf-8"),
            (CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// Extract the session token from a `Cookie` header value, if present.
fn session_token(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(COOKIE)?.to_str().ok()?;
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

/// Resolve the optional viewer: usable sessions identify, while missing,
/// stale, or broken sessions read as anonymous — never a refusal here.
async fn optional_viewer(state: &PublicHtmlState, headers: &HeaderMap) -> Option<uuid::Uuid> {
    let token = session_token(headers)?;
    match authenticate(&state.pool, &token).await {
        Ok(Some(account)) => Some(account.user.id),
        _ => None,
    }
}

/// Render one demand as safe HTML: 200 with full or limited content, or
/// one generic unavailable page. Same-host origin is not required: safe
/// GET responses carry no credentials and no private data.
async fn page(
    State(state): State<PublicHtmlState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let viewer_id = optional_viewer(&state, &headers).await;
    match project_request(&state.pool, viewer_id, id).await {
        Ok(PublicProjection::Full(full)) => html_response(
            StatusCode::OK,
            render_full(
                &full.title,
                &full.category_code,
                full.budget_cents,
                &full.condition,
                &full.city_code,
                &full.region_code,
                &full.author_name,
            ),
        ),
        Ok(PublicProjection::Limited(limited)) => {
            html_response(StatusCode::OK, render_limited(&limited.status))
        }
        Ok(PublicProjection::Unavailable) | Err(_) => {
            html_response(StatusCode::NOT_FOUND, render_unavailable())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_covers_both_contexts() {
        assert_eq!(escape_html("&<>\"'"), "&amp;&lt;&gt;&quot;&#x27;");
        assert_eq!(escape_html("plain 600.00"), "plain 600.00");
    }

    #[test]
    fn malicious_title_renders_as_text() {
        let rendered = render_full(
            "<script>alert(1)</script>",
            "home_appliances",
            60_000,
            "either",
            "campinas",
            "centro",
            "Buyer",
        );
        assert!(!rendered.contains("<script>"));
        assert!(rendered.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn generic_pages_carry_no_slots() {
        for rendered in [render_limited("completed"), render_unavailable()] {
            assert!(!rendered.contains("{{"));
            assert!(!rendered.contains("}}"));
        }
        assert!(render_unavailable().contains("Unavailable request"));
    }
}
