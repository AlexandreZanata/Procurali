-- 0018_feedback: one structured observation set per contacted pair/cycle.
--
-- After a recorded contact, the buyer may optionally answer whether the
-- seller actually had the offered item ("yes", "no", or "unable to
-- confirm"). The window opens 24 hours after first contact and closes 14
-- days after it; corrections land inside the same window with preserved
-- revision history. One set exists per contact — and contacts are already
-- unique per buyer, seller, request, cycle, and offer — so repeat
-- handoffs and revisions can never multiply credit. A negative answer
-- never becomes a validated report or a ban by itself; it only invites
-- an optional separate report through the existing flow.
--
-- Privacy: answers reference the business fact only. There is
-- deliberately no phone, reporter-identity-beyond-the-filing-buyer,
-- token, secret, plaintext destination, or score column — now or by
-- migration convention.

CREATE TABLE contact_feedback (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    contact_id UUID NOT NULL REFERENCES contacts (id) ON DELETE RESTRICT,
    buyer_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    seller_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    request_id UUID NOT NULL REFERENCES requests (id) ON DELETE RESTRICT,
    cycle_number INTEGER NOT NULL,
    answer TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT contact_feedback_answer_known CHECK (
        answer IN ('yes', 'no', 'unable')
    ),
    CONSTRAINT contact_feedback_version_positive CHECK (version >= 1),
    CONSTRAINT contact_feedback_one_set UNIQUE (contact_id)
);

CREATE TABLE feedback_revisions (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    feedback_id UUID NOT NULL REFERENCES contact_feedback (id) ON DELETE RESTRICT,
    previous_answer TEXT,
    answer TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT feedback_revisions_answer_known CHECK (
        answer IN ('yes', 'no', 'unable')
    ),
    CONSTRAINT feedback_revisions_previous_known CHECK (
        previous_answer IS NULL OR previous_answer IN ('yes', 'no', 'unable')
    )
);

CREATE INDEX feedback_revisions_feedback_lookup
    ON feedback_revisions (feedback_id, created_at, id);
