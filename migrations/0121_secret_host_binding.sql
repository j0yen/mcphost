-- compat: previous -- additive nullable column; an old release never reads it.
-- mcphost 0121_secret_host_binding: PRD-mcphost-secret-non-exfiltration-proof R1.
--
-- hosts_json: JSON array of lower-case hostnames the secret may be sent to,
-- or NULL for an unbound secret (today's behaviour).
ALTER TABLE secrets ADD COLUMN hosts_json TEXT;
