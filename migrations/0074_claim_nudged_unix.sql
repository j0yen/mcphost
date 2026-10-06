-- compat: previous -- one additive nullable column on `tenants`; an old
-- release simply never queries it, no existing row's shape changes.
-- mcphost 0074_claim_nudged_unix: PRD-mcphost-ownership-moment requirement
-- 2 (AC2): the one-time "hand this claim link to your human" nudge on an
-- unclaimed external tenant's first successful `host.tool_publish` --
-- `Db`'s own atomic `UPDATE ... WHERE claim_nudged_unix IS NULL` is what
-- makes "never twice" hold under concurrent calls, the same single-winner
-- shape `Db::verify_claim_code`'s `owner_verified_at IS NULL` guard already
-- uses.
ALTER TABLE tenants ADD COLUMN claim_nudged_unix INTEGER;
