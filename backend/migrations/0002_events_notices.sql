-- 0002_events_notices: durable business facts and in-product notices.
--
-- One business mutation, its event(s), and its notice(s) commit atomically from
-- the caller's transaction: repositories take an executor, so the application
-- layer passes its open transaction. A failure anywhere rolls back everything;
-- no applied version is ever half-recorded.
--
-- Privacy (INV-31): these tables hold identifiers, timestamps, kinds, and
-- bounded non-sensitive payloads. There is deliberately no phone, address,
-- reporter, token, or secret column — now or by migration convention.
-- Payloads stay small (8 KB backstop) and notice bodies stay short (1000
-- scalar values backstop); Rust-side checks enforce the same bounds first.
-- Deduplication: one notice per (event, recipient) — repeats create new
-- events, so legitimate repeats are never blocked while retries duplicate
-- nothing (INV-34). History is append-only: no UPDATE/DELETE paths exist here.

CREATE TABLE business_events (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor_id UUID,
    resource_kind TEXT NOT NULL,
    resource_id UUID NOT NULL,
    cycle INTEGER,
    revision UUID,
    effective_at TIMESTAMPTZ NOT NULL,
    kind TEXT NOT NULL,
    policy TEXT NOT NULL DEFAULT 'mvp-free',
    source TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    CONSTRAINT business_events_payload_bounded
        CHECK (octet_length(payload::text) <= 8192)
);

CREATE INDEX business_events_resource_lookup
    ON business_events (resource_kind, resource_id);

CREATE TABLE notices (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    account_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    kind TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    resource_id UUID NOT NULL,
    event_id UUID NOT NULL REFERENCES business_events (id) ON DELETE RESTRICT,
    body TEXT NOT NULL,
    acknowledged_at TIMESTAMPTZ,
    CONSTRAINT notices_body_bounded CHECK (char_length(body) <= 1000),
    CONSTRAINT notices_one_per_event_recipient UNIQUE (event_id, account_id)
);

CREATE INDEX notices_recipient_lookup
    ON notices (account_id, acknowledged_at);
