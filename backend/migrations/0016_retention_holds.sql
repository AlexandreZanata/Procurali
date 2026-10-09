-- 0016_retention_holds: classified retention with reviewed holds.
--
-- Proposed business defaults (privacy-and-security 15.3, pending launch
-- confirmation — represented here, never as legal determinations):
-- ordinary removed-content and contact context keep access-limited records
-- for up to 90 days after closure or deletion; open safety incidents keep
-- only incident-relevant evidence with necessity reviews every 90 days;
-- closed safety evidence keeps a 180-day maximum after decision; the
-- moderation decision trail keeps the minimum explainable record;
-- commercial-record retention stays future-only with no permanent
-- default. A recorded exception names its basis, scope, owner, and
-- review/end condition.
--
-- One open hold per subject, class, and standing (INV-34): repeats
-- converge instead of duplicating. No silent permanence exists: statuses
-- are open or released only, expiry changes nothing by itself, and only
-- explicit review or release writes move a hold. Physical cleanup belongs
-- to the later worker, which reads open holds past due.
--
-- Privacy (INV-31, INV-44): holds reference identifiers, classes,
-- purposes, due dates, and decision metadata only. There is deliberately
-- no phone, address, reporter, token, secret, plaintext destination, or
-- accusation column — now or by migration convention.

CREATE TABLE retention_holds (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    subject_kind TEXT NOT NULL,
    subject_id UUID NOT NULL,
    class TEXT NOT NULL,
    purpose TEXT NOT NULL,
    due_at TIMESTAMPTZ NOT NULL,
    basis TEXT,
    review_condition TEXT,
    owner_id UUID REFERENCES users (id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'open',
    created_by UUID REFERENCES users (id) ON DELETE RESTRICT,
    reviewed_at TIMESTAMPTZ,
    reviewed_by UUID REFERENCES users (id) ON DELETE RESTRICT,
    released_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT retention_holds_subject_known CHECK (
        subject_kind IN ('user', 'request', 'offer', 'contact', 'report', 'case')
    ),
    CONSTRAINT retention_holds_class_known CHECK (
        class IN ('ordinary', 'incident', 'audit', 'exception')
    ),
    CONSTRAINT retention_holds_purpose_bounded CHECK (
        char_length(purpose) BETWEEN 1 AND 500
    ),
    CONSTRAINT retention_holds_status_known CHECK (
        status IN ('open', 'released')
    ),
    CONSTRAINT retention_holds_exception_documented CHECK (
        class <> 'exception'
        OR (
            basis IS NOT NULL AND char_length(basis) BETWEEN 1 AND 500
            AND review_condition IS NOT NULL
            AND char_length(review_condition) BETWEEN 1 AND 500
            AND owner_id IS NOT NULL
        )
    )
);

CREATE UNIQUE INDEX retention_holds_one_open_row
    ON retention_holds (subject_kind, subject_id, class)
    WHERE status = 'open';

CREATE INDEX retention_holds_due_lookup
    ON retention_holds (status, due_at, id);

CREATE INDEX retention_holds_subject_lookup
    ON retention_holds (subject_kind, subject_id);
