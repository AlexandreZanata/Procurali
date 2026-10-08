-- 0005_professional_profile: declared commercial classification, free of charge.
--
-- Professional sellers declare business activity with a display name, a type
-- from the fixed commercial vocabulary, and an approximate locality. The
-- account stays the same person-controlled account with the same privacy
-- rules; casual individuals keep free basic access (INV-39, AC-44, EC-18).
--
-- This table deliberately has no subscription, plan, badge, verification, or
-- volume column: classification is a free self-declaration, payment cannot
-- grant powers (INV-06), and volume alone never classifies. Paid Radar
-- entitlements, if any, live in their own future ledger — never here.
-- Withdrawal stamps `withdrawn_at` instead of deleting the row, so ending a
-- declaration (or ending payment, which never touches this table) keeps
-- ownership and history intact. Transitions append business events; nothing
-- here rewrites history.

CREATE TABLE professional_profiles (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    user_id UUID NOT NULL UNIQUE REFERENCES users (id) ON DELETE RESTRICT,
    business_name TEXT NOT NULL,
    business_type TEXT NOT NULL,
    city TEXT NOT NULL,
    region TEXT NOT NULL,
    withdrawn_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT professional_business_name_bounded CHECK (char_length(business_name) BETWEEN 1 AND 80),
    CONSTRAINT professional_business_type_known CHECK (business_type IN ('shop', 'merchant', 'reseller', 'business', 'recurring')),
    CONSTRAINT professional_city_bounded CHECK (char_length(city) BETWEEN 1 AND 120),
    CONSTRAINT professional_region_bounded CHECK (char_length(region) BETWEEN 1 AND 40)
);
