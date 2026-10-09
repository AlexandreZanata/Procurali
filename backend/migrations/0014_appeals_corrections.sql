-- 0014_appeals_corrections: bounded appeals with traceable corrections.
--
-- One appeal per appellant and decided case (INV-34: repeats converge
-- instead of multiplying), with later material evidence appended as its
-- own rows and every outcome recorded as upheld, narrowed, or reversed
-- (spec 16.4). Corrections travel as separate business facts so derived
-- reputation and metrics stay traceable (INV-42); the original allegation,
-- decision, and outcome rows are never rewritten here.
--
-- Privacy (INV-31): appeal rows reference the grouped case and the filing
-- appellant only. Other reporters in the case never enter an appeal row,
-- and there is deliberately no phone, address, token, secret, plaintext
-- destination, or accusation column — now or by migration convention.
-- Appeal reads return the appeal row alone, never member reports.

CREATE TABLE appeals (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    case_id UUID NOT NULL REFERENCES report_cases (id) ON DELETE RESTRICT,
    appellant_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    grounds TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    decided_by UUID REFERENCES users (id) ON DELETE RESTRICT,
    decided_at TIMESTAMPTZ,
    decision_reason TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT appeals_grounds_bounded CHECK (
        char_length(grounds) BETWEEN 1 AND 1000
    ),
    CONSTRAINT appeals_status_known CHECK (
        status IN ('open', 'upheld', 'narrowed', 'reversed')
    ),
    CONSTRAINT appeals_decision_reason_bounded CHECK (
        char_length(decision_reason) <= 1000
    ),
    CONSTRAINT appeals_one_per_appellant UNIQUE (case_id, appellant_id)
);

CREATE INDEX appeals_case_lookup
    ON appeals (case_id, created_at, id);

CREATE INDEX appeals_appellant_lookup
    ON appeals (appellant_id);

CREATE TABLE appeal_evidence (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    appeal_id UUID NOT NULL REFERENCES appeals (id) ON DELETE RESTRICT,
    submitted_by UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    statement TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT appeal_evidence_statement_bounded CHECK (
        char_length(statement) BETWEEN 1 AND 1000
    )
);

CREATE INDEX appeal_evidence_appeal_lookup
    ON appeal_evidence (appeal_id, created_at, id);
