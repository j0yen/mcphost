-- compat: previous -- additive columns with defaults; an old release never reads them.
-- mcphost 0119_publish_visibility_tags: PRD-mcphost-publish-visibility-and-tags R2.
--
-- tags: JSON array of lower-case catalog tags. tested_version: the
-- tool_versions.version whose publish-time dry run last passed (NULL = never).
ALTER TABLE tools ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE tools ADD COLUMN tested_version INTEGER;
