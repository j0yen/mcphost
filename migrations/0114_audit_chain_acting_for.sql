-- compat: previous -- three nullable additive columns on `audit_chain`; an old
-- release never selects them, and every existing record keeps its original
-- hash because an absent field is left out of the hashed payload.
-- mcphost 0111_audit_chain_acting_for: PRD-mcphost-acting-for-in-ledger-and-audit R2/R3.
--
-- `action` / `detail` carry a control-plane event (a `revoke_cascade` record);
-- `acting_for` is the JSON lineage of the delegated key that made the call.
ALTER TABLE audit_chain ADD COLUMN action TEXT;
ALTER TABLE audit_chain ADD COLUMN detail TEXT;
ALTER TABLE audit_chain ADD COLUMN acting_for TEXT;
