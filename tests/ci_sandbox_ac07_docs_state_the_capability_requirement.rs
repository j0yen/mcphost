//! PRD-mcphost-ci-sandbox-coverage
//! AC7 (P2) -- Given the repo docs, When a contributor reads local-test
//! requirements, Then the userns capability requirement and probe behavior
//! are stated.

use crate::ci_sandbox_support;
use ci_sandbox_support as support;

#[test]
fn the_readme_states_the_capability_requirement_and_the_probe_behaviour() {
    let readme = support::read_repo_file("README.md");

    assert!(
        readme.contains("user namespace"),
        "README must name the user-namespace requirement"
    );
    assert!(
        readme.contains("unshare --user --map-root-user -- true"),
        "README must give the exact command a contributor can run to check the \
         capability -- the same one sandbox::supports_user_namespaces() runs"
    );
    assert!(
        readme.contains("kernel.apparmor_restrict_unprivileged_userns=0")
            || readme.contains("kernel.unprivileged_userns_clone=1"),
        "README must name the fix, not only the requirement"
    );
    assert!(
        readme.contains("require_user_namespaces_or_ci_skip"),
        "README must point at the guard whose doc comment holds the full contract"
    );
    assert!(
        readme.contains(".github/workflows/ci.yml"),
        "README must point at how CI grants the same capability, so a \
         contributor can tell local behaviour from CI behaviour"
    );
}
