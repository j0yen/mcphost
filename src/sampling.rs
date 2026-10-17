//! Error codes raised by sampling-from-tools, declared where they are raised
//! (PRD-mcphost-error-codes-generated-registry).

use crate::err_code;

err_code! {
    /// A single run asked the client's model more times than its call ceiling.
    pub SAMPLING_CALLS_EXCEEDED,
    code: "sampling_calls_exceeded", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Ask the client's model fewer times per run; data.limit is the per-run sampling-call ceiling.",
}

err_code! {
    /// The tenant's daily sampling token quota is spent.
    pub SAMPLING_QUOTA_EXCEEDED,
    code: "sampling_quota_exceeded", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Wait for the daily reset or move to a plan with a larger sampling_tokens_per_day.",
}

err_code! {
    /// The client's model did not answer a sampling request in time.
    pub SAMPLING_TIMEOUT,
    code: "sampling_timeout", family: Call, class: AgentFault, retryable: true,
    remedy: "Retry with a smaller max_tokens or a shorter prompt; the client's model did not answer in time.",
}

err_code! {
    /// The connected client does not support `sampling/createMessage`.
    pub SAMPLING_UNSUPPORTED,
    code: "sampling_unsupported", family: Call, class: HostRefusal, retryable: false,
    remedy: "Do not retry; this client cannot sample. Use data.alternative or call from a client that supports sampling.",
}
