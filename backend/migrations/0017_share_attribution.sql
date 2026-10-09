-- 0017_share_attribution: anonymous landings with marker attribution.
--
-- Shared-link landings record an eligible source and time only: the
-- visited demand when identifiable, nothing at all for direct visits.
-- No visitor identity, recipient group, message content, or device data
-- is collected here — attribution later resolves through a client-held
-- landing marker (never server-side identity linkage), and missing or
-- ambiguous markers stay unknown instead of guessed.
--
-- The seven-day acquisition window matches the initial request lifecycle
-- (sharing 13.4); landings never expire by themselves and no cleanup
-- touches them on this card. Privacy (INV-31): identifiers and instants
-- only — no phone, reporter, token, secret, or destination column, now
-- or by migration convention.

CREATE TABLE share_landings (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    request_id UUID REFERENCES requests (id) ON DELETE RESTRICT,
    landed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT share_landings_landed_bounded CHECK (landed_at <= now() + interval '1 minute')
);

CREATE INDEX share_landings_request_lookup
    ON share_landings (request_id, landed_at, id);
