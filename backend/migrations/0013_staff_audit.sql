-- 0013_staff_audit: scoped staff grants and purpose audit.
--
-- Operational powers live here, apart from every marketplace identity
-- (INV-05, INV-06, INV-39: buyer, seller, professional classification, and
-- paid access never imply, carry, or buy staff authority — grants attach
-- explicitly to accounts, and use-sites recheck live state every time).
-- There is deliberately no default privileged account: this migration
-- creates tables only, zero grant rows, so the first grant must arrive
-- through the audited bootstrap path with explicit operator input.
--
-- `staff_grants` holds one live row per account, role, and scope:
-- moderator (scoped safety work) or administrator (moderation abilities
-- plus bans, reversals, permission management, and policy approval).
-- The granter is nullable because the launch bootstrap precedes any
-- administrator; every later grant names its granting administrator.
--
-- `staff_access_audit` records the INV-37/AC-39 shape for important
-- administrative changes (actor, reason, scope, time, previous/resulting
-- eligibility) and the actor-plus-purpose row for every sensitive
-- inspection. Grant and revoke decisions audit into the same table so no
-- privileged change happens silently.
--
-- Privacy (INV-31): these tables hold identifiers, roles, scopes, reasons,
-- purposes, and state transitions only. There is deliberately no phone,
-- address, reporter, token, secret, plaintext destination, accusation, or
-- trust-label column — now or by migration convention. Audit rows are
-- staff-private facts, never public projections.

CREATE TABLE staff_grants (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    role TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'safety',
    reason TEXT NOT NULL,
    granted_by UUID REFERENCES users (id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT staff_grants_role_known CHECK (role IN ('moderator', 'administrator')),
    CONSTRAINT staff_grants_scope_known CHECK (scope IN ('safety', 'policy')),
    CONSTRAINT staff_grants_reason_bounded CHECK (
        char_length(reason) BETWEEN 1 AND 1000
    ),
    CONSTRAINT staff_grants_one_live_row UNIQUE (user_id, role, scope)
);

CREATE INDEX staff_grants_user_lookup
    ON staff_grants (user_id);

CREATE TABLE staff_access_audit (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    actor_id UUID NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    action TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id UUID NOT NULL,
    purpose TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    scope TEXT,
    previous_state TEXT,
    resulting_state TEXT,
    policy_version TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT staff_access_audit_action_known CHECK (
        action IN ('inspect', 'grant', 'revoke')
    ),
    CONSTRAINT staff_access_audit_target_known CHECK (
        target_kind IN ('user', 'request', 'offer', 'report', 'case', 'staff_grant')
    ),
    CONSTRAINT staff_access_audit_purpose_bounded CHECK (
        char_length(purpose) BETWEEN 1 AND 500
    ),
    CONSTRAINT staff_access_audit_reason_bounded CHECK (
        char_length(reason) <= 1000
    ),
    CONSTRAINT staff_access_audit_policy_bounded CHECK (
        char_length(policy_version) BETWEEN 1 AND 32
    )
);

CREATE INDEX staff_access_audit_actor_lookup
    ON staff_access_audit (actor_id, created_at);

CREATE INDEX staff_access_audit_target_lookup
    ON staff_access_audit (target_kind, target_id);
