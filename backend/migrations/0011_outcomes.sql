-- 0011_outcomes: declared buyer outcomes with attribution and corrections.
--
-- One declared answer per call, latest wins: completion, cancellation, or
-- explicit unknown. Platform results link only to a historical contact the
-- buyer actually made (INV-43: unattributed completions credit nobody, and
-- no fictional contact is ever created); foreign or uncontacted offers can
-- receive no credit. A repeated identical answer returns the standing row
-- instead of duplicating closure (INV-34); a changed answer supersedes the
-- prior row, which stays readable as the recorded correction reference.
-- Lifecycle itself still moves through the closure transition exactly once.
--
-- Outcome vocabulary mirrors the buyer's answers (`completed`,
-- `cancelled`, `unresolved`); sources stay declared labels (`platform`,
-- `elsewhere`, `unknown`) with no verification implied. No phone, address,
-- reporter, token, or secret column exists here — attribution travels as
-- identifiers only.

CREATE TABLE request_outcomes (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    request_id UUID NOT NULL REFERENCES requests (id) ON DELETE RESTRICT,
    outcome TEXT NOT NULL,
    source TEXT,
    attributed_offer_id UUID REFERENCES offers (id) ON DELETE RESTRICT,
    supersedes UUID REFERENCES request_outcomes (id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT request_outcomes_outcome_known CHECK (outcome IN ('completed', 'cancelled', 'unresolved')),
    CONSTRAINT request_outcomes_source_known CHECK (
        source IS NULL OR source IN ('platform', 'elsewhere', 'unknown')
    ),
    CONSTRAINT request_outcomes_attribution_scoped CHECK (
        (source = 'platform' AND attributed_offer_id IS NOT NULL)
        OR (source IS DISTINCT FROM 'platform' AND attributed_offer_id IS NULL)
    )
);

CREATE INDEX request_outcomes_request_lookup
    ON request_outcomes (request_id, created_at, id);
