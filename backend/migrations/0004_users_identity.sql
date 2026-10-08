-- 0004_users_identity: account identity, credential digests, and the block ledger.
--
-- One account owns at most one current phone number (INV-07, AC-03, EC-22):
-- `users_current_phone_unique` enforces uniqueness of the keyed phone lookup
-- across non-deleted accounts, so application checks and the database agree.
-- Deleting an account stamps `deleted_at` without removing its row: the
-- identifier stays distinct from a later new account while the same number
-- becomes assignable again (number recycling, reviewed handling in its owning
-- card). A partial unique index — not a table-wide constraint — is what makes
-- both properties hold at once.
--
-- Privacy (INV-31): phones live here only as authenticated ciphertext plus a
-- keyed HMAC-SHA256 lookup (pgcrypto, vetted primitives, keys passed per call
-- and never stored). Sessions and challenges persist only as Argon2 digests
-- computed by the repository (authentication protocol); plaintext tokens and
-- codes never reach a column. There is deliberately no phone, token, code,
-- address, reporter, or secret column — now or by migration convention.
-- History is append-only for identity facts: no UPDATE path exists except the
-- narrow lifecycle transitions (delete, revoke, consume) owned by later cards.
--
-- `user_blocks` lands here as a minimal real relationship ledger so early
-- offer/contact guards query an existing empty table; block commands and
-- cascades arrive in P10.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    display_name TEXT NOT NULL,
    city TEXT NOT NULL,
    region TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    policy_version TEXT NOT NULL,
    policy_accepted_at TIMESTAMPTZ NOT NULL,
    phone_ciphertext BYTEA NOT NULL,
    phone_lookup TEXT NOT NULL,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT users_display_name_bounded CHECK (char_length(display_name) BETWEEN 1 AND 80),
    CONSTRAINT users_city_bounded CHECK (char_length(city) BETWEEN 1 AND 120),
    CONSTRAINT users_region_bounded CHECK (char_length(region) BETWEEN 1 AND 40),
    CONSTRAINT users_state_known CHECK (state IN ('pending', 'active', 'suspended', 'banned', 'deleted')),
    CONSTRAINT users_policy_version_bounded CHECK (char_length(policy_version) BETWEEN 1 AND 32)
);

CREATE UNIQUE INDEX users_current_phone_unique
    ON users (phone_lookup) WHERE deleted_at IS NULL;

CREATE TABLE sessions (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    session_digest TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX sessions_user_lookup
    ON sessions (user_id);

CREATE TABLE phone_challenges (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    phone_lookup TEXT NOT NULL,
    challenge_digest TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT phone_challenges_attempts_bounded CHECK (attempts >= 0)
);

CREATE INDEX phone_challenges_lookup
    ON phone_challenges (phone_lookup);

CREATE TABLE user_blocks (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    blocker_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    blocked_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT user_blocks_no_self CHECK (blocker_id <> blocked_id),
    CONSTRAINT user_blocks_one_direction UNIQUE (blocker_id, blocked_id)
);
