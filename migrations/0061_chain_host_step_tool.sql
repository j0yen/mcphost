-- compat: previous -- one additive `calls.step_tool` column; nothing
-- existing changes shape.
-- mcphost 0061_chain_host_step_tool: PRD-mcphost-chain-host-steps P1
-- requirement 6 (AC6) -- the allowlisted `host.*` verb a chain's own
-- `calls` row dispatched as a step, so `host.usage {by: "tool"}` keeps
-- attributing the call to the chain's own tool name (one row, same as
-- today) while this column lets a caller see which host step ran inside
-- it. NULL for every call that is not a chain dispatching exactly one
-- host step (an ordinary tool call, or a chain with no host step).
ALTER TABLE calls ADD COLUMN step_tool TEXT;
