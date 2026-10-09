-- 0010_contacts: private contact initiations with frozen snapshots.
--
-- A contact is an immutable contextual initiation, never a transaction
-- (INV-33: nothing here claims delivery, conversation, or sale). Each row
-- freezes the business context both parties acted on: the requirement and
-- terms snapshots plus the effective initiation time, so later edits never
-- rewrite what was seen (INV-24), alongside the request and offer revision
-- numbers that scope it.
--
-- The destination is history, not an address book (INV-29): only a frozen
-- copy of the seller's phone ciphertext at initiation is kept — never a
-- plaintext number, never a live lookup — so later phone changes cannot
-- rewrite past handoffs and no unrestricted user-phone lookup exists. It
-- lives for administrative, purpose-limited use only and never enters any
-- serialized shape (INV-30, INV-31).
--
-- Identity is idempotent (INV-34): every handoff attempt carries its
-- caller-supplied identity exactly once, and the first initiation per
-- buyer/seller/request-cycle/offer combination is the unique contact —
-- later valid handoffs are repeat events, never second unique rows. Entry
-- source records where the buyer chose contact (offer detail, comparison,
-- or future surfaces) in bounded free text.

CREATE TABLE contacts (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    handoff_id UUID NOT NULL,
    buyer_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    seller_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    request_id UUID NOT NULL REFERENCES requests (id) ON DELETE RESTRICT,
    cycle_number INTEGER NOT NULL,
    offer_id UUID NOT NULL REFERENCES offers (id) ON DELETE RESTRICT,
    request_revision_number INTEGER NOT NULL,
    offer_terms_number INTEGER NOT NULL,
    request_title TEXT NOT NULL,
    request_budget_cents BIGINT NOT NULL,
    offer_description TEXT NOT NULL,
    offer_price_cents BIGINT NOT NULL,
    destination_ciphertext BYTEA NOT NULL,
    entry_source TEXT NOT NULL,
    initiated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT contacts_handoff_unique UNIQUE (handoff_id),
    CONSTRAINT contacts_unique_contact UNIQUE (buyer_id, seller_id, request_id, cycle_number, offer_id),
    CONSTRAINT contacts_target_cycle FOREIGN KEY (request_id, cycle_number)
        REFERENCES request_cycles (request_id, cycle_number) ON DELETE RESTRICT,
    CONSTRAINT contacts_numbers_positive CHECK (
        cycle_number >= 1
        AND request_revision_number >= 1
        AND offer_terms_number >= 1
    ),
    CONSTRAINT contacts_title_bounded CHECK (char_length(request_title) BETWEEN 1 AND 120),
    CONSTRAINT contacts_budget_positive CHECK (request_budget_cents > 0),
    CONSTRAINT contacts_description_bounded CHECK (char_length(offer_description) BETWEEN 1 AND 120),
    CONSTRAINT contacts_price_positive CHECK (offer_price_cents > 0),
    CONSTRAINT contacts_entry_bounded CHECK (char_length(entry_source) BETWEEN 1 AND 64)
);

CREATE INDEX contacts_request_lookup
    ON contacts (request_id, cycle_number);

CREATE INDEX contacts_offer_lookup
    ON contacts (offer_id);
