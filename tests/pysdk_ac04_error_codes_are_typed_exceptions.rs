//! PRD-mcphost-python-sdk-generated AC4 (P0) -- Given a test host that returns
//! `args_invalid` with `data.argument="spec"`, When the SDK receives it, Then
//! it raises `errors.ArgsInvalid` with `.code == "args_invalid"`,
//! `.data.argument == "spec"` and `isinstance(e, MCPHostError)`; an unknown
//! code raises `MCPHostError` itself with the code preserved.

use crate::pysdk;

#[test]
fn pysdk_ac04_args_invalid_is_typed_and_unknown_code_is_the_base_class() {
    pysdk::pytest_ok("tests/test_errors.py");
}

/// The exception set is generated from the host's own code registries, so a
/// code the host can emit but the SDK lacks cannot exist.
#[test]
fn pysdk_ac04_every_host_error_code_has_a_generated_class() {
    let errors_py = std::fs::read_to_string(pysdk::sdk_dir().join("mcphost_sdk/errors.py")).unwrap();
    let codes = mcphost::sdk_python::all_error_codes();
    assert!(codes.contains(&"args_invalid".to_string()));
    for code in &codes {
        let class = mcphost::sdk_python::camel(code);
        assert!(errors_py.contains(&format!("class {class}(MCPHostError)")), "no class for {code}");
    }
}
