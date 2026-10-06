//! PRD-mcphost-docs-one-url-flow
//! AC1 (P0) -- Given the served `/llms.txt`, When its "Quickstart for
//! agents" section is read, Then its first three steps are connect, first
//! call, save `onboarding.url`, and `signup`/`host.redeem` appear only
//! under the explicit-signup heading.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const QUICKSTART_HEADING: &str = "## Quickstart for agents";
const EXPLICIT_SIGNUP_HEADING: &str = "## Explicit signup";
const NEXT_HEADING: &str = "## Leaving";

fn quickstart_section() -> &'static str {
    let start = LLMS_TXT
        .find(QUICKSTART_HEADING)
        .expect("www/llms.txt must have a 'Quickstart for agents' heading");
    let end = LLMS_TXT[start..]
        .find(EXPLICIT_SIGNUP_HEADING)
        .map(|offset| start + offset)
        .expect("'Explicit signup' heading must follow 'Quickstart for agents'");
    &LLMS_TXT[start..end]
}

fn explicit_signup_section() -> &'static str {
    let start = LLMS_TXT
        .find(EXPLICIT_SIGNUP_HEADING)
        .expect("www/llms.txt must have an 'Explicit signup' heading");
    let end = LLMS_TXT[start..]
        .find(NEXT_HEADING)
        .map(|offset| start + offset)
        .expect("'## Leaving' must follow 'Explicit signup'");
    &LLMS_TXT[start..end]
}

/// First three numbered steps of the Quickstart section: connect, make the
/// first call, save `onboarding.url`.
#[test]
fn first_three_steps_are_connect_first_call_save_url() {
    let section = quickstart_section();
    let step1 = section.find("1. ").expect("step 1 present");
    let step2 = section.find("2. ").expect("step 2 present");
    let step3 = section.find("3. ").expect("step 3 present");
    let step4 = section.find("4. ").expect("step 4 present");
    assert!(step1 < step2 && step2 < step3 && step3 < step4, "steps out of order");

    let step1_text = &section[step1..step2];
    let step2_text = &section[step2..step3];
    let step3_text = &section[step3..step4];

    assert!(
        step1_text.contains("Connect to") && step1_text.contains("mcphost.dev/mcp"),
        "step 1 must be connecting to the endpoint: {step1_text}"
    );
    assert!(
        step2_text.to_lowercase().contains("first call"),
        "step 2 must be making the first call: {step2_text}"
    );
    assert!(
        step3_text.contains("onboarding.url") && step3_text.to_lowercase().contains("save"),
        "step 3 must be saving onboarding.url: {step3_text}"
    );
}

#[test]
fn quickstart_section_never_calls_signup_or_host_redeem() {
    let section = quickstart_section();
    assert!(
        !section.contains("signup("),
        "the Quickstart section must not call signup(): {section}"
    );
    assert!(
        !section.contains("host.redeem"),
        "the Quickstart section must not mention host.redeem: {section}"
    );
}

#[test]
fn signup_and_host_redeem_appear_under_the_explicit_signup_heading() {
    let explicit = explicit_signup_section();
    assert!(
        explicit.contains("signup(name)"),
        "the explicit-signup section must document signup(name): {explicit}"
    );
    assert!(
        explicit.contains("host.redeem"),
        "the explicit-signup section must document host.redeem: {explicit}"
    );
}
