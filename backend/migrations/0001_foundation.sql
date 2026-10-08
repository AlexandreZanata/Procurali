-- 0001_foundation: migration machinery proof and native UUIDv7 defaults.
--
-- This migration exists so the runner, version tracking, and PostgreSQL-native
-- UUIDv7 generation are proven before any repository relies on them. Entity
-- tables (accounts, requests, offers, ...) arrive in their owning phase cards
-- and follow the same `UUID PRIMARY KEY DEFAULT uuidv7()` pattern.
--
-- Privileges: migrations run with an owner-capable URL (see
-- `persistence::migrations` docs). Runtime DML grants for least-privilege
-- roles land with the entity migrations, never here.

CREATE TABLE foundation_ids (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
