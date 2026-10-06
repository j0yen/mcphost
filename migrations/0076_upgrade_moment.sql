-- compat: previous -- three additive nullable columns (one on `tenants`,
-- two on `billing_events`); an old release simply never reads any of
-- them, no existing statement's result set changes.
-- mcphost 0076_upgrade_moment: PRD-mcphost-upgrade-moment requirements 3
-- (AC4), 6 (AC7).
--
-- `tenants.paid_test_unix` is `paid_unix`'s test-mode sibling: stamped
-- (once, first time only) when a test-mode `checkout.session.completed`
-- webhook is processed for this tenant, so the fleet's own weekly
-- test-mode checkout never counts as a real payment (`paid_unix` stays
-- null) while still being observable.
--
-- `billing_events.source` carries which refusal (or `manual`) started the
-- checkout behind a ledgered event -- read off the Stripe session's own
-- `metadata[source]` for a real `checkout.session.completed`/
-- `checkout.started` row, or set directly by `billing::checkout` at
-- session-creation time.
--
-- `billing_events.session_id` is the Stripe Checkout Session id --
-- `billing::checkout`'s own self-ledgered `checkout.started` row (AC7)
-- and the webhook's `checkout.session.completed` row for the same
-- session share this value, so `admin.upgrades` can correlate "started"
-- with "completed" (or, absent that, "expired") without a dedicated
-- checkout-attempts table.
ALTER TABLE tenants ADD COLUMN paid_test_unix INTEGER;
ALTER TABLE billing_events ADD COLUMN source TEXT;
ALTER TABLE billing_events ADD COLUMN session_id TEXT;
