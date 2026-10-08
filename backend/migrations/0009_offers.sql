-- 0009_offers: offer slots and immutable terms history.
--
-- One seller slot per request cycle (INV-18: at most one offer per seller
-- and cycle, and withdrawal or rejection never frees another slot — the
-- UNIQUE below makes a second slot unrepresentable while rows are never
-- deleted). Every offer names its existing request, cycle, and revision
-- (INV-02) through composite foreign keys, so orphan offers, phantom
-- cycles, and phantom revisions cannot persist.
--
-- Terms are append-only like requirement revisions (INV-24): `offer_terms`
-- rows are never updated, and the offer row promotes current terms with a
-- monotonic counter. Money is integer minor units with a positive backstop
-- (INV-19); the within-budget rule is submission policy enforced by the
-- offer writers, not by this schema. Conditions are `new` or `used` only:
-- `either` is a request-side acceptance, never an offer term. Seller
-- locality is a frozen snapshot (plain codes, no catalog reference), so a
-- later locality change never rewrites what the seller declared.
--
-- Text bounds reuse the established shapes (titles 1..120, notes 0..500):
-- no dedicated canonical bound exists for offer descriptions yet, and a
-- future one refines these columns without moving data. Lifecycle follows
-- state-transitions 6.2 (`sent` first; engagement and terminal states
-- afterwards); visibility separates business state from moderation.
-- Seller-unequal-buyer (INV-03) is application-checked in the writer — a
-- CHECK cannot compare across tables — alongside the durable keys below.
--
-- Privacy (INV-31): identifiers, terms, lifecycle, and timing only. No
-- phone, ciphertext, address, reporter, token, or secret column exists on
-- these tables, now or by migration convention.

CREATE TABLE offers (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    request_id UUID NOT NULL REFERENCES requests (id) ON DELETE RESTRICT,
    cycle_number INTEGER NOT NULL,
    revision_number INTEGER NOT NULL,
    seller_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    description TEXT NOT NULL,
    price_cents BIGINT NOT NULL,
    condition TEXT NOT NULL,
    city_code TEXT NOT NULL,
    region_code TEXT NOT NULL,
    notes TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'sent',
    visibility TEXT NOT NULL DEFAULT 'visible',
    terminal_reason TEXT,
    current_terms_number INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT offers_target_cycle FOREIGN KEY (request_id, cycle_number)
        REFERENCES request_cycles (request_id, cycle_number) ON DELETE RESTRICT,
    CONSTRAINT offers_target_revision FOREIGN KEY (request_id, revision_number)
        REFERENCES request_revisions (request_id, revision_number) ON DELETE RESTRICT,
    CONSTRAINT offers_one_slot_per_seller_cycle UNIQUE (request_id, cycle_number, seller_id),
    CONSTRAINT offers_description_bounded CHECK (char_length(description) BETWEEN 1 AND 120),
    CONSTRAINT offers_notes_bounded CHECK (char_length(notes) <= 500),
    CONSTRAINT offers_price_positive CHECK (price_cents > 0),
    CONSTRAINT offers_condition_known CHECK (condition IN ('new', 'used')),
    CONSTRAINT offers_state_known CHECK (state IN ('sent', 'viewed', 'contacted', 'withdrawn', 'rejected', 'expired', 'invalidated', 'suspended')),
    CONSTRAINT offers_visibility_known CHECK (visibility IN ('visible', 'hidden')),
    CONSTRAINT offers_terms_nonnegative CHECK (current_terms_number >= 0)
);

CREATE INDEX offers_request_lookup
    ON offers (request_id, cycle_number);

CREATE INDEX offers_seller_lookup
    ON offers (seller_id);

CREATE TABLE offer_terms (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    offer_id UUID NOT NULL REFERENCES offers (id) ON DELETE RESTRICT,
    terms_number INTEGER NOT NULL,
    description TEXT NOT NULL,
    price_cents BIGINT NOT NULL,
    condition TEXT NOT NULL,
    city_code TEXT NOT NULL,
    region_code TEXT NOT NULL,
    notes TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT offer_terms_number_positive CHECK (terms_number >= 1),
    CONSTRAINT offer_terms_description_bounded CHECK (char_length(description) BETWEEN 1 AND 120),
    CONSTRAINT offer_terms_notes_bounded CHECK (char_length(notes) <= 500),
    CONSTRAINT offer_terms_price_positive CHECK (price_cents > 0),
    CONSTRAINT offer_terms_condition_known CHECK (condition IN ('new', 'used')),
    CONSTRAINT offer_terms_one_per_number UNIQUE (offer_id, terms_number)
);

CREATE INDEX offer_terms_offer_lookup
    ON offer_terms (offer_id, terms_number);
