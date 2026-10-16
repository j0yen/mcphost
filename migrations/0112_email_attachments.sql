-- compat: previous -- additive columns with defaults on an existing table; an
-- old release never queries them and every old row reads as a plain put.
-- mcphost 0109_email_attachments: PRD-mcphost-email-attachments-as-blobs R1.
--
-- A mail-borne blob records where it came from: `source = 'email'`, the
-- provider's message key and the part's index (together unique per tenant, so
-- a redelivered mail cannot store a part twice), the mime the sender declared,
-- and whether the part was an inline image.
ALTER TABLE blobs ADD COLUMN source TEXT;
ALTER TABLE blobs ADD COLUMN message_id TEXT;
ALTER TABLE blobs ADD COLUMN part_index INTEGER;
ALTER TABLE blobs ADD COLUMN declared_mime TEXT;
ALTER TABLE blobs ADD COLUMN inline INTEGER NOT NULL DEFAULT 0;
CREATE UNIQUE INDEX IF NOT EXISTS idx_blobs_email_part
    ON blobs(tenant_id, message_id, part_index) WHERE message_id IS NOT NULL;
