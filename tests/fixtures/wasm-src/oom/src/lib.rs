#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

impl Guest for Component {
    // AC3 fixture: grows a buffer's reserved capacity in a loop until the
    // host's memory limiter refuses a `memory.grow` request -- which the
    // host turns into a hard trap regardless of whether this used a
    // fallible or infallible allocation API (`try_reserve_exact` is used
    // anyway, for cleaner guest-level semantics: the host's own trap always
    // wins the race before this could ever see an `Err` itself). Any
    // wasm32-wasip1 component that touches the heap links the full
    // `wasi:cli` world regardless -- see `MAX_COMPONENT_BYTES`'s doc.
    fn call(_args: String) -> Result<String, String> {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            if buf.try_reserve_exact(1024 * 1024).is_err() {
                return Err("allocation failed".to_string());
            }
            let new_len = buf.capacity();
            buf.resize(new_len, 0);
            std::hint::black_box(&buf);
        }
    }
}

bindings::export!(Component with_types_in bindings);
