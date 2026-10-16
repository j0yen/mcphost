//! PRD-mcphost-jq-kind
//! AC3 -- Given `program: ".rows | map(.price) | add"` and 1 KiB of args,
//! When called 100 times, Then p95 <= 5 ms and the result is the sum.

use mcphost::kinds::jq::JqKind;
use mcphost::kinds::{CallCtx, Kind};
use serde_json::{Value, json};

#[tokio::test]
async fn jqk_ac03_p95_of_a_one_kib_call_is_within_five_ms() {
    let rows: Vec<Value> = (0..28).map(|i| json!({"sku": format!("sku-{i:04}-widget"), "price": i + 1})).collect();
    let args = json!({"rows": rows});
    let size = args.to_string().len();
    assert!((1000..=1400).contains(&size), "args are about 1 KiB, got {size} bytes");
    let want: i64 = (1..=28).sum();

    let kind = JqKind;
    let spec = json!({"program": ".rows | map(.price) | add"});
    kind.validate(&spec).expect("the program is valid");
    let ctx = CallCtx::for_test(1, "t_jqk_ac03");

    crate::perf_budget!(5, {
        let got = kind.call(&spec, args.clone(), &ctx).await.expect("the call succeeds");
        assert_eq!(got, json!(want), "the result is the sum of the prices");
    });
}
