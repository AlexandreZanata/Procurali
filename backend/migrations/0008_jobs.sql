-- 0008_jobs: durable bounded background-job claims.
--
-- Same-package background work (expiry sweeps, notice fan-out, and their
-- successors) executes through claimed job rows instead of in-memory
-- queues, so a worker crash never loses eligible work and two workers never
-- complete the same claim twice (INV-34: repeats duplicate nothing).
--
-- Lifecycle: `queued` (due when `not_before` passes) → `claimed` (one
-- `claimed_by` worker holds the row until `lease_expires_at`) →
-- `completed`, or back to `queued` with a later `not_before` on retryable
-- failure, or to `failed` with a static `last_error` on permanent failure
-- or exhaustion. `attempts` counts claims; `max_attempts` bounds them. A
-- claim whose lease lapsed without settlement is reclaimable by any worker
-- (crash recovery); rows that can never be claimed again are swept to
-- `failed` explicitly — nothing silently stalls, and terminal rows are
-- never resurrected by a later claim.
--
-- Privacy: payloads carry identifiers, numbers, and timestamps only. Phone
-- destinations, tokens, secrets, and session material are refused at enqueue
-- by the worker module (no phone/token/secret-shaped content is
-- representable here by convention, enforced in code, not by a CHECK that
-- could not see JSON shapes reliably). Failure reasons are static strings;
-- no input values are ever rendered into `last_error`.

CREATE TABLE background_jobs (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    kind TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    state TEXT NOT NULL DEFAULT 'queued',
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    not_before TIMESTAMPTZ NOT NULL DEFAULT now(),
    claimed_by TEXT,
    lease_expires_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    CONSTRAINT jobs_kind_bounded CHECK (char_length(kind) BETWEEN 1 AND 64),
    CONSTRAINT jobs_state_known CHECK (state IN ('queued', 'claimed', 'completed', 'failed')),
    CONSTRAINT jobs_attempts_nonnegative CHECK (attempts >= 0),
    CONSTRAINT jobs_max_attempts_positive CHECK (max_attempts >= 1),
    CONSTRAINT jobs_lease_paired CHECK (
        (state = 'claimed' AND claimed_by IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR (state <> 'claimed' AND claimed_by IS NULL AND lease_expires_at IS NULL)
    ),
    CONSTRAINT jobs_terminal_complete CHECK (
        (state = 'completed' AND completed_at IS NOT NULL)
        OR (state <> 'completed' AND completed_at IS NULL)
    )
);

CREATE INDEX background_jobs_poll_lookup
    ON background_jobs (state, not_before, created_at, id);

CREATE INDEX background_jobs_lease_lookup
    ON background_jobs (state, lease_expires_at)
    WHERE state = 'claimed';
