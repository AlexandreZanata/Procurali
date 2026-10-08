-- 0003_idempotency: durable per-action deduplication records.
--
-- One business action, its event(s), and its idempotency record commit atomically
-- from the caller's transaction: repositories take an executor, so the application
-- layer passes its open transaction. A failure anywhere rolls back everything;
-- a rolled-back attempt reserves no key (the row vanishes with the transaction),
-- so the same key remains usable for a genuine retry.
--
-- Scope (INV-34, EC-25): exactly one record per (actor, operation, key).
-- Same key + same input digest replays the existing result reference; same key +
-- different digest is refused by the application as `idempotency_key_reuse` and
-- inserts nothing. New attempts never overwrite history (append-only: no
-- UPDATE/DELETE paths exist here).
--
-- Privacy: this table holds identifiers and digests only. There is deliberately
-- no phone, address, reporter, token, destination, or secret column — now or by
-- migration convention. The result reference is a private event identifier, never
-- a reusable publicly cached destination: replay callers must revalidate current
-- eligibility (INV-28, INV-38) before responding. Key/digest bounds keep rows small;
-- Rust-side checks enforce the same bounds first.

CREATE TABLE idempotency_records (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    actor_id UUID NOT NULL,
    operation TEXT NOT NULL,
    key TEXT NOT NULL,
    input_digest TEXT NOT NULL,
    result_event UUID NOT NULL REFERENCES business_events (id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT idempotency_operation_bounded
        CHECK (char_length(operation) BETWEEN 1 AND 64),
    CONSTRAINT idempotency_key_bounded
        CHECK (char_length(key) BETWEEN 1 AND 128),
    CONSTRAINT idempotency_digest_bounded
        CHECK (char_length(input_digest) BETWEEN 1 AND 128),
    CONSTRAINT idempotency_one_key_per_actor_operation
        UNIQUE (actor_id, operation, key)
);

CREATE INDEX idempotency_actor_lookup
    ON idempotency_records (actor_id, operation);
