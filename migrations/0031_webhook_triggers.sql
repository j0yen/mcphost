-- compat: previous -- one additive column (`triggers.hook_id`, ALTER TABLE
-- ADD COLUMN) plus one additive index and one wholly new table
-- (`webhook_dedupe`); an old release simply never queries any of the
-- three, no existing row's shape changes, no existing statement's result
-- set changes (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0031_webhook_triggers: PRD-mcphost-webhook-inbox P0 requirements
-- 1-6.
--
-- The fourth `triggers.kind` value, `'webhook'`: no `CHECK` constraint ever
-- pinned `kind`, so this is purely additive -- `config_json` for a webhook
-- trigger is `{"name": <inbox table suffix>, "verify": "hmac"|"none"|
-- "stripe", "secret_enc": <hex>, "secret_nonce": <hex>}` (see
-- `webhooks.rs`'s `build_webhook_config`), the same "kind decides the
-- shape, config_hash backs the row's own uniqueness" pattern 0015's
-- schedule/event and 0023's message triggers already use.
--
-- `hook_id`: the opaque `POST /hook/<hook_id>` path segment (technical
-- considerations: "not the tenant id"; a fresh `new_ulid()`, unrelated to
-- the trigger's own id so a leaked URL never also leaks which trigger row
-- it maps to). `idx_triggers_hook_id` is the one-indexed-lookup the
-- unauthenticated route needs to resolve straight to `(tenant, trigger)`
-- without a table scan. Nullable and not `UNIQUE` at the SQL level (every
-- other kind leaves it `NULL` forever) -- uniqueness for the ids this
-- code actually assigns is already guaranteed by `new_ulid()`'s own
-- 128-bit randomness, the same trust `triggers.id`/`runs.id` already place
-- in `new_ulid()` elsewhere in this schema.
ALTER TABLE triggers ADD COLUMN hook_id TEXT;

CREATE INDEX IF NOT EXISTS idx_triggers_hook_id ON triggers(hook_id);

-- `webhook_dedupe`: requirement 6 / AC7's row-level idempotency -- a
-- repeated `X-Mcphost-Delivery-Id` within 24h must not re-insert a row at
-- all (unlike `event_dedupe`, migration 0017, which dedupes a *run*, not a
-- stored row -- a webhook trigger stores every accepted delivery as an
-- `inbox_<name>` row even when paused, so the row insert itself is what
-- needs deduping). Same `PRIMARY KEY (trigger_id, dedupe_key)` claim shape
-- as `event_dedupe`; `Db::claim_webhook_dedupe` reuses that migration's own
-- `EVENT_DEDUPE_WINDOW_S` (86,400s) constant for the "within 24h" freshness
-- window rather than a second one, since both mean exactly the same thing.
CREATE TABLE IF NOT EXISTS webhook_dedupe (
    trigger_id   TEXT NOT NULL REFERENCES triggers(id) ON DELETE CASCADE,
    dedupe_key   TEXT NOT NULL,
    created_unix INTEGER NOT NULL,
    PRIMARY KEY (trigger_id, dedupe_key)
);

CREATE INDEX IF NOT EXISTS idx_webhook_dedupe_created ON webhook_dedupe(created_unix);
