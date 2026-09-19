#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

impl Guest for Component {
    // AC1/AC5/AC8/AC10 fixture: returns exactly the JSON it was given, so
    // callers can use call args as the expected output shape (including
    // declared-output promotion fixtures).
    fn call(args: String) -> Result<String, String> {
        Ok(args)
    }
}

bindings::export!(Component with_types_in bindings);
