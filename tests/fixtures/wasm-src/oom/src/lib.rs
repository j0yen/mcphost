#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

impl Guest for Component {
    // AC3 fixture: allocates in a growing loop until the host's memory
    // limiter refuses further `memory.grow` -- the host converts that
    // refusal into a hard trap, so this never returns normally.
    fn call(_args: String) -> Result<String, String> {
        let mut blobs: Vec<Vec<u8>> = Vec::new();
        loop {
            blobs.push(std::hint::black_box(vec![0u8; 1024 * 1024]));
        }
    }
}

bindings::export!(Component with_types_in bindings);
