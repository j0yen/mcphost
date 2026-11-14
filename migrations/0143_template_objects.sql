-- compat: previous -- one wholly new table; no existing column or table is altered, and an old release
-- ignores it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0143_template_objects: PRD-mcphost-tenant-templates R2/R3/R6.
--
-- `template_objects`: the `template: <name>` tag of every object `host.template_apply` created for a
-- tenant, one row per `(tenant, template, obj_kind, obj_name)`. `obj_kind` is `tool`, `table` or
-- `trigger`; `version` is the tool version the apply left active (NULL for the other kinds), which is
-- how `host.template_remove` tells an object still unmodified since apply from one the user changed;
-- `object_id` is the trigger id for a trigger row. Tools, tables and triggers keep their own schemas.
CREATE TABLE IF NOT EXISTS template_objects (
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    template     TEXT NOT NULL,
    obj_kind     TEXT NOT NULL,
    obj_name     TEXT NOT NULL,
    version      INTEGER,
    object_id    TEXT,
    applied_unix INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, template, obj_kind, obj_name)
);
