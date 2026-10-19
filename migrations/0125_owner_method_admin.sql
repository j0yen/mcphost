-- compat: previous -- the owner_method CHECK set only grows; an old release writes email/github/dns, all still legal.
-- mcphost 0125_owner_method_admin: PRD-mcphost-email-send-live-proof R5.
--
-- admin.tenant_verify_owner is a fourth entry in VERIFICATION_METHODS
-- (src/verify.rs), so the owner_method CHECK gains 'admin'. SQLite cannot alter
-- a CHECK in place: the column is rebuilt through a scratch column so no value
-- is lost. The CHECK set is the VERIFICATION_METHODS names (a drift test
-- compares them).
ALTER TABLE tenants ADD COLUMN owner_method_next TEXT;
UPDATE tenants SET owner_method_next = owner_method;
ALTER TABLE tenants DROP COLUMN owner_method;
ALTER TABLE tenants ADD COLUMN owner_method TEXT CHECK (owner_method IN ('email', 'github', 'dns', 'admin'));
UPDATE tenants SET owner_method = owner_method_next;
ALTER TABLE tenants DROP COLUMN owner_method_next;
