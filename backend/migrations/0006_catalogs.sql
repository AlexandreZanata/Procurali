-- 0006_catalogs: stable category and locality identities for requests.
--
-- Six allowed launch categories with scope descriptions (requests 8.6):
-- Furniture, Home Appliances, Electronics, Bicycles, Baby and Children's
-- Items, Tools. Real estate, motor vehicles, jobs, services, food,
-- medication, and animals are excluded: no rows exist for them here, and no
-- client label can activate one. Each category carries a lifecycle status
-- (INV-08): `allowed` for new use, `retired` (existing cycles may finish,
-- new use refused), `prohibited` (never usable).
--
-- Cities and regions are keyed identities, not coordinates: there are
-- deliberately no latitude/longitude columns — now or by migration
-- convention — and no account data. Regions hang under their city with a
-- composite key, so same-named regions in different cities stay distinct.
-- Cities serve traffic only when explicitly enabled; the migration seeds no
-- cities (operations enable them explicitly, tests use synthetic fixtures).
-- Label changes never touch keys (EC-35): renames keep codes, and scope
-- changes version through new rows rather than rewriting meaning.

CREATE TABLE catalog_categories (
    code TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    scope TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'allowed',
    CONSTRAINT catalog_category_code_bounded CHECK (char_length(code) BETWEEN 1 AND 32),
    CONSTRAINT catalog_category_label_bounded CHECK (char_length(label) BETWEEN 1 AND 120),
    CONSTRAINT catalog_category_scope_bounded CHECK (char_length(scope) BETWEEN 1 AND 280),
    CONSTRAINT catalog_category_status_known CHECK (status IN ('allowed', 'retired', 'prohibited'))
);

INSERT INTO catalog_categories (code, label, scope, status) VALUES
    ('furniture', 'Furniture', 'Tables, chairs, sofas, beds, and other household furniture.',
     'allowed'),
    ('home_appliances', 'Home Appliances', 'Refrigerators, stoves, washers, and other home appliances.',
     'allowed'),
    ('electronics', 'Electronics', 'Phones, computers, TVs, and other consumer electronics. Account credentials and restricted items stay prohibited within this category.',
     'allowed'),
    ('bicycles', 'Bicycles', 'Bicycles and non-motorized cycling goods. Motor vehicles are excluded.',
     'allowed'),
    ('baby_kids', 'Baby and Children''s Items', 'Strollers, toys, and other permitted physical goods for babies and children. Services and restricted medical products are excluded.',
     'allowed'),
    ('tools', 'Tools', 'Hand and power tools for home and trade use.',
     'allowed');

CREATE TABLE catalog_cities (
    code TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    CONSTRAINT catalog_city_code_bounded CHECK (char_length(code) BETWEEN 1 AND 32),
    CONSTRAINT catalog_city_name_bounded CHECK (char_length(name) BETWEEN 1 AND 120)
);

CREATE TABLE catalog_regions (
    city_code TEXT NOT NULL REFERENCES catalog_cities (code) ON DELETE RESTRICT,
    code TEXT NOT NULL,
    name TEXT NOT NULL,
    CONSTRAINT catalog_region_code_bounded CHECK (char_length(code) BETWEEN 1 AND 32),
    CONSTRAINT catalog_region_name_bounded CHECK (char_length(name) BETWEEN 1 AND 120),
    CONSTRAINT catalog_region_identity PRIMARY KEY (city_code, code)
);
