//! Share intent and prepared messages: link freely, claim nothing.
//!
//! Canonical rules: sharing 13.1 with 13.4 (prepare sharing only for
//! currently shareable demand; record observable intent per channel —
//! WhatsApp, copy, or device — without claiming delivery and without
//! collecting recipients; attribution stays separate), INV-09 (only
//! active eligible demand shares — terminal rows and restricted rows
//! refuse), INV-31 (prepared messages carry the public item, budget, and
//! region only — no phone, notes, offers, reporter, token, or secret
//! material), INV-33 with AC-33 (one sharing-action fact is intent, never
//! a contact and never a delivered message), and EC-15 with EC-37 (old
//! links resolve the current live row at open time — nothing is frozen
//! into the link, and stale previews carry no authority).
//!
//! Links are stable identifier paths to the public page: resolving them
//! always re-projects, so a later suspension, deletion, or block changes
//! what the same link shows.

use serde::Serialize;

use crate::application::public_request::{project_request, PublicProjection};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::requests::format_budget;

/// Share-intent channels: where the user takes the prepared message.
pub const SHARE_CHANNELS: [&str; 3] = ["whatsapp", "copy", "device"];

/// One prepared share: the stable link plus its safe message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SharePackage {
    /// Shared demand identifier.
    pub request_id: uuid::Uuid,
    /// Stable link path resolving current state at open time.
    pub link: String,
    /// Prepared plain-text message with public context only.
    pub message: String,
    /// Recorded intent channel.
    pub channel: String,
}

/// Typed share failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareError {
    /// Unknown channel or malformed input.
    InvalidField,
    /// No shareable projection exists for this viewer and identifier.
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid share field"),
            Self::NotFound => f.write_str("share target not found"),
            Self::StorageFailed => f.write_str("share storage failed"),
        }
    }
}

impl std::error::Error for ShareError {}

/// Prepare one share for an optional viewer: the demand must project full
/// details right now, and the recorded fact names the intent channel —
/// never delivery, never recipients, of which this writer keeps none.
///
/// # Errors
///
/// Returns [`ShareError::InvalidField`] for unknown channels,
/// [`ShareError::NotFound`] for missing, terminal, restricted, or blocked
/// rows, else [`ShareError::StorageFailed`]. Reasons are static.
pub async fn prepare_share(
    pool: &sqlx::PgPool,
    viewer_id: Option<uuid::Uuid>,
    request_id: uuid::Uuid,
    channel: &str,
) -> Result<SharePackage, ShareError> {
    if !SHARE_CHANNELS.contains(&channel) {
        return Err(ShareError::InvalidField);
    }
    let full = match project_request(pool, viewer_id, request_id)
        .await
        .map_err(|_| ShareError::NotFound)?
    {
        PublicProjection::Full(full) => full,
        _ => return Err(ShareError::NotFound),
    };
    let link = format!("/api/v1/public/requests/{request_id}/html");
    let message = format!(
        "{} — Budget {} — {}, {}",
        full.title,
        format_budget(full.budget_cents),
        full.region_code,
        full.city_code,
    );
    let mut tx = pool.begin().await.map_err(|_| ShareError::StorageFailed)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: viewer_id,
            resource_kind: "request",
            resource_id: request_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "share.intended",
            policy: "mvp-free",
            source: "api",
            payload: serde_json::json!({
                "channel": channel,
            }),
        },
    )
    .await
    .map_err(|_| ShareError::StorageFailed)?;
    tx.commit().await.map_err(|_| ShareError::StorageFailed)?;
    Ok(SharePackage {
        request_id,
        link,
        message,
        channel: channel.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_channels_are_closed() {
        for channel in ["whatsapp", "copy", "device"] {
            assert!(SHARE_CHANNELS.contains(&channel));
        }
        assert!(!SHARE_CHANNELS.contains(&"sms"));
        assert!(!SHARE_CHANNELS.contains(&"broadcast"));
    }

    #[test]
    fn share_package_carries_public_context_only() {
        let package = SharePackage {
            request_id: uuid::Uuid::now_v7(),
            link: "/api/v1/public/requests/00000000-0000-7000-8000-000000000000/html".to_owned(),
            message: "Refrigerator — Budget 600.00 — centro, campinas".to_owned(),
            channel: "copy".to_owned(),
        };
        let rendered = serde_json::to_string(&package).expect("package serializes");
        assert!(rendered.contains("Refrigerator"));
        for absent in [
            "phone",
            "lookup",
            "cipher",
            "token",
            "notes",
            "offers",
            "reporter",
            "detail",
            "secret",
            "recipient",
        ] {
            assert!(!rendered.contains(absent), "no {absent} in package");
        }
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ShareError::InvalidField,
            ShareError::NotFound,
            ShareError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
