-- 0012_reports_cases: report and case intake schema.
--
-- An allegation is stored, never judged here (INV-36: reports remain
-- allegations until assessed; raw volume alone cannot cause a permanent
-- ban). Each `reports` row is one reporter's allegation with its reason,
-- target, and bounded detail; each `report_cases` row is the grouped
-- incident identity those allegations belong to. The two counts stay
-- independent on purpose: assessing a case never rewrites its reports,
-- and withdrawing or duplicating a report never assesses its case.
--
-- Privacy (INV-31): reporter identity lives only on the private report
-- row for purpose-limited staff review. It never enters a target-facing
-- projection, a public page, or a share message. There is deliberately no
-- phone, address, token, secret, plaintext destination, public accusation,
-- verdict, or guilt column on either table — now or by migration
-- convention. Snapshots below carry only the permitted business context
-- the reporter acted on (titles/descriptions already visible or
-- historically interacted with), never an accusation.
--
-- Retention (EC-14, EC-38): targets are referenced by kind plus identifier
-- without a hard foreign key, alongside an immutable submitted snapshot.
-- Closing a request, withdrawing an offer, or soft-deleting an account
-- (rows stay, `deleted_at` stamped) never erases the allegation or its
-- snapshot; staff review continues from retained evidence while public
-- reuse stays closed. Reporter and case links use ON DELETE RESTRICT so a
-- hard removal cannot silently drop incident evidence. Snapshots have no
-- UPDATE path: only status columns transition.

CREATE TABLE report_cases (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    status TEXT NOT NULL DEFAULT 'open',
    severity TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT report_cases_status_known CHECK (
        status IN ('open', 'review', 'valid', 'invalid', 'duplicate', 'withdrawn')
    ),
    CONSTRAINT report_cases_severity_known CHECK (
        severity IS NULL OR severity IN ('critical', 'high', 'normal', 'low')
    )
);

CREATE TABLE reports (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    case_id UUID NOT NULL REFERENCES report_cases (id) ON DELETE RESTRICT,
    reporter_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    target_kind TEXT NOT NULL,
    target_id UUID NOT NULL,
    reason TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'open',
    target_title_snapshot TEXT NOT NULL,
    target_context_snapshot TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT reports_target_kind_known CHECK (
        target_kind IN ('request', 'offer', 'user')
    ),
    CONSTRAINT reports_reason_known CHECK (
        reason IN (
            'fraud',
            'spam',
            'false_information',
            'prohibited_item',
            'inappropriate_behavior',
            'inappropriate_content',
            'other'
        )
    ),
    CONSTRAINT reports_status_known CHECK (
        status IN ('open', 'review', 'valid', 'invalid', 'duplicate', 'withdrawn')
    ),
    CONSTRAINT reports_detail_bounded CHECK (char_length(detail) <= 1000),
    CONSTRAINT reports_title_snapshot_bounded CHECK (
        char_length(target_title_snapshot) BETWEEN 1 AND 120
    ),
    CONSTRAINT reports_context_snapshot_bounded CHECK (
        char_length(target_context_snapshot) <= 1000
    )
);

CREATE INDEX reports_case_lookup
    ON reports (case_id, created_at, id);

CREATE INDEX reports_reporter_lookup
    ON reports (reporter_id);

CREATE INDEX reports_target_lookup
    ON reports (target_kind, target_id);
