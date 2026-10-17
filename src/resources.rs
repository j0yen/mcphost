//! Error codes raised by the MCP resources surface, declared where they are
//! raised (PRD-mcphost-error-codes-generated-registry).

use crate::err_code;

err_code! {
    /// A tenant already holds its plan's maximum number of exposed resources.
    pub RESOURCES_MAX,
    code: "resources_max", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Remove a resource or move to a plan with a higher resources ceiling; data.limit is the ceiling.",
}
