-- compat: previous -- one wholly new table (CREATE TABLE IF NOT EXISTS,
-- same precedent as migrations 0011/0014/0026/0044/0043); an old release
-- simply never queries it, and nothing about its existing behavior changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0065_public_tool_url: PRD-mcphost-public-tool-url P0 requirements
-- 1, 8.
-- (Renumbered from this PRD's own 0059, then 0062, during rebase:
-- mcphost-url-bound-tenants claimed 0059 first, then mcphost-one-next-tool
-- claimed 0060, then mcphost-chain-host-steps claimed 0061, then
-- mcphost-chain-run-lineage claimed 0062, then mcphost-table-concept-graph
-- claimed 0063, then mcphost-trigger-set-idempotent claimed 0064, all
-- landing on main ahead of this branch.)
--
-- `tool_share_tokens`: one row per (tenant, tool) currently shared with
-- `visibility = 'url'`. `token_hash` (sha256 hex of the plaintext bearer
-- token, AC1's own "stored hashed") is what `GET/POST /x/{token}/{tool}`
-- looks up by -- one indexed query, never a table scan. `token_enc`/
-- `token_nonce` are the same plaintext, AEAD-encrypted at rest with this
-- process's own `SecretBox` (the same mechanism `webhooks.rs`'s own
-- trigger secret already uses) -- needed because requirement 1's "a second
-- identical `host.tool_share` call returns the same URL" means the
-- plaintext must be recoverable, unlike a webhook secret (which is shown
-- once and never again). `UNIQUE(tenant_id, tool_name)` is what makes
-- re-sharing idempotent (a second `host.tool_share(visibility="url")`
-- finds this row instead of minting a second token for the same tool);
-- `UNIQUE(token_hash)` backs the lookup's own uniqueness. Revocation
-- (`host.tool_unshare`, or `host.tool_share` to any other visibility) is a
-- plain `DELETE` of this row -- a revoked token and a token that was never
-- issued then resolve through the exact same "no row" path, which is what
-- gives AC4's constant-time guarantee for free rather than needing a
-- separate `revoked_unix` branch to cost the same as the found branch.
CREATE TABLE IF NOT EXISTS tool_share_tokens (
    id           TEXT PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool_name    TEXT NOT NULL,
    token_hash   TEXT NOT NULL,
    token_enc    BLOB NOT NULL,
    token_nonce  BLOB NOT NULL,
    created_unix INTEGER NOT NULL,
    UNIQUE(tenant_id, tool_name),
    UNIQUE(token_hash)
);

CREATE INDEX IF NOT EXISTS idx_tool_share_tokens_hash ON tool_share_tokens(token_hash);
