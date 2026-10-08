-- 0007_requests: request cycle and revision schema.
--
-- One purchase intent (requests 8.1) carries exactly one author (INV-01,
-- ownership never transfers here: no UPDATE path touches `author_id`), its
-- money/condition/category/location (INV-08, INV-10, INV-11), and a lifecycle
-- state plus an independent visibility (state-transitions 6.1: business state
-- and moderation visibility coexist; removal hides without erasing history).
--
-- History is append-only and monotonic (INV-12, INV-14, entities 5.2-5.3):
-- `request_revisions` snapshots the requirements sellers responded to
-- (revision numbers increase; rows are never updated), `request_cycles`
-- records each seven-day activation (cycle numbers increase; the request keeps
-- its original publication time across renewals — renewal never rewrites it).
-- Foreign keys to `requests` (ON DELETE RESTRICT) make orphan cycles or
-- revisions unrepresentable; category/city/region references keep catalog
-- identity stable (INV-08, INV-11, INV-22, EC-35).
--
-- Money is integer minor units (cents) with a positive backstop (INV-10, no
-- universal maximum per principles 2.3). Budgets render as exact decimal
-- strings on the wire; binary floating point appears nowhere.
--
-- Privacy (INV-31): these tables hold identifiers, requirements, lifecycle,
-- and timing only. There is deliberately no phone, phone ciphertext/lookup,
-- address, reporter, token, code, or secret column — now or by migration
-- convention. Public reads project an allowlist; the phone destination lives
-- only in `users` and never derives into request serialization.

CREATE TABLE requests (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    author_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    title TEXT NOT NULL,
    category_code TEXT NOT NULL REFERENCES catalog_categories (code) ON DELETE RESTRICT,
    budget_cents BIGINT NOT NULL,
    condition TEXT NOT NULL,
    city_code TEXT NOT NULL REFERENCES catalog_cities (code) ON DELETE RESTRICT,
    region_code TEXT NOT NULL,
    notes TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'draft',
    visibility TEXT NOT NULL DEFAULT 'private',
    original_published_at TIMESTAMPTZ,
    current_cycle_number INTEGER NOT NULL DEFAULT 0,
    current_revision_number INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT requests_title_bounded CHECK (char_length(title) BETWEEN 1 AND 120),
    CONSTRAINT requests_notes_bounded CHECK (char_length(notes) <= 500),
    CONSTRAINT requests_budget_positive CHECK (budget_cents > 0),
    CONSTRAINT requests_condition_known CHECK (condition IN ('new', 'used', 'either')),
    CONSTRAINT requests_state_known CHECK (state IN ('draft', 'active', 'completed', 'expired', 'cancelled', 'suspended')),
    CONSTRAINT requests_visibility_known CHECK (visibility IN ('private', 'public', 'hidden')),
    CONSTRAINT requests_cycle_nonnegative CHECK (current_cycle_number >= 0),
    CONSTRAINT requests_revision_nonnegative CHECK (current_revision_number >= 0),
    CONSTRAINT requests_region_in_city FOREIGN KEY (city_code, region_code)
        REFERENCES catalog_regions (city_code, code) ON DELETE RESTRICT
);

CREATE INDEX requests_author_lookup
    ON requests (author_id);

CREATE TABLE request_revisions (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    request_id UUID NOT NULL REFERENCES requests (id) ON DELETE RESTRICT,
    revision_number INTEGER NOT NULL,
    title TEXT NOT NULL,
    category_code TEXT NOT NULL REFERENCES catalog_categories (code) ON DELETE RESTRICT,
    budget_cents BIGINT NOT NULL,
    condition TEXT NOT NULL,
    city_code TEXT NOT NULL REFERENCES catalog_cities (code) ON DELETE RESTRICT,
    region_code TEXT NOT NULL,
    notes TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT request_revisions_number_positive CHECK (revision_number >= 1),
    CONSTRAINT request_revisions_title_bounded CHECK (char_length(title) BETWEEN 1 AND 120),
    CONSTRAINT request_revisions_notes_bounded CHECK (char_length(notes) <= 500),
    CONSTRAINT request_revisions_budget_positive CHECK (budget_cents > 0),
    CONSTRAINT request_revisions_condition_known CHECK (condition IN ('new', 'used', 'either')),
    CONSTRAINT request_revisions_one_per_number UNIQUE (request_id, revision_number),
    CONSTRAINT request_revisions_region_in_city FOREIGN KEY (city_code, region_code)
        REFERENCES catalog_regions (city_code, code) ON DELETE RESTRICT
);

CREATE INDEX request_revisions_request_lookup
    ON request_revisions (request_id, revision_number);

CREATE TABLE request_cycles (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    request_id UUID NOT NULL REFERENCES requests (id) ON DELETE RESTRICT,
    cycle_number INTEGER NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deadline TIMESTAMPTZ NOT NULL,
    ended_at TIMESTAMPTZ,
    CONSTRAINT request_cycles_number_positive CHECK (cycle_number >= 1),
    CONSTRAINT request_cycles_one_per_number UNIQUE (request_id, cycle_number),
    CONSTRAINT request_cycles_deadline_after_start CHECK (deadline > started_at)
);

CREATE INDEX request_cycles_request_lookup
    ON request_cycles (request_id, cycle_number);
