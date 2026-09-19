#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

impl Guest for Component {
    // AC3 fixture: spins forever with a genuine side effect (via
    // black_box) so the compiler can't prove the loop away -- must be
    // stopped by the host's fuel/epoch budget, not by returning on its own.
    fn call(_args: String) -> Result<String, String> {
        let mut x: u64 = 0;
        loop {
            x = std::hint::black_box(x.wrapping_add(1));
        }
    }
}

bindings::export!(Component with_types_in bindings);
