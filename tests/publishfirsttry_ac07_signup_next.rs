//! P1 AC7 -- Given `signup`, When it succeeds, Then the result carries
//! `next: "host.quickstart"`.

use crate::common;
use common::{TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn signup_result_points_at_quickstart() {
    let server = TestServer::start().await;
    let client = common::McpClient::new(&server.base_url);

    let result = client
        .tools_call("signup", json!({"name": "Fresh Agent"}))
        .await
        .expect("signup");
    let structured = extract_structured(&result);

    assert_eq!(
        structured["next"],
        json!("host.quickstart"),
        "signup result must carry next: \"host.quickstart\": {structured}"
    );
}
