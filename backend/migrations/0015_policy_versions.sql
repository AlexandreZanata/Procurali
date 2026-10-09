-- 0015_policy_versions: effective business-policy versions.
--
-- Category standing changes and prospective limits arrive under an
-- approved, effective-dated policy version — never by rewriting catalog
-- rows' meaning (EC-35: renames keep codes, scope changes version) and
-- never by rewriting past facts (INV-42, AC-50: historical events keep the
-- version and category they recorded). A version dated in the future
-- changes nothing until it becomes effective; writers resolve the version
-- row and refuse not-yet-effective references.
--
-- The launch set ships as `v1`, effective since this database began —
-- the same version every existing fact already names. Later versions
-- arrive only through the audited staff operation, which also records the
-- creating administrator.
--
-- Privacy: versions hold identifiers, dates, and business text only. No
-- account, phone, reporter, token, or secret column exists here.

CREATE TABLE policy_versions (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    version TEXT NOT NULL,
    effective_from TIMESTAMPTZ NOT NULL,
    description TEXT NOT NULL,
    created_by UUID REFERENCES users (id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT policy_versions_version_bounded CHECK (
        char_length(version) BETWEEN 1 AND 32
    ),
    CONSTRAINT policy_versions_description_bounded CHECK (
        char_length(description) BETWEEN 1 AND 1000
    ),
    CONSTRAINT policy_versions_one_row_per_version UNIQUE (version)
);

INSERT INTO policy_versions (version, effective_from, description, created_by)
VALUES ('v1', now(), 'Launch policy set.', NULL);

CREATE INDEX policy_versions_effective_lookup
    ON policy_versions (effective_from, version);
