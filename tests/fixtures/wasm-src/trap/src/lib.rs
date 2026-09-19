#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

impl Guest for Component {
    // AC6 fixture: an explicit `unreachable` trap, distinct from a fuel/
    // memory-limit trap, with a distinguishing message on stderr so
    // host.tool_logs has something to carry.
    fn call(_args: String) -> Result<String, String> {
        core::arch::wasm32::unreachable()
    }
}

bindings::export!(Component with_types_in bindings);
