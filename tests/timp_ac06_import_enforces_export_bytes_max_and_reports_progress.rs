//! PRD-mcphost-export-tables-and-import-restore
//! AC6 -- Given a free-plan tenant and an archive whose manifest totals
//! 60 MB, When `host.import` runs, Then `quota_exceeded {export_bytes_max}`
//! is returned before any row exists; Given a 1,000-row table and 10 docs
//! within quota, Then import completes in ≤ 30 s and `host.runs.get`
//! reported `entries_done` rising.

use crate::common;
use crate::timp;

use std::time::Duration;

use common::TestServer;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use tar::{Builder, Header};

fn tar_gz(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    for (path, data) in entries {
        let mut header = Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, &data[..]).expect("append");
    }
    builder.into_inner().expect("tar").finish().expect("gzip")
}

#[tokio::test]
async fn a_60_mb_manifest_is_refused_on_the_free_plan_before_anything_is_written() {
    let server = TestServer::start().await;
    let t = timp::new_tenant(&server, "Timp AC6 Quota").await;
    // A real, completed export of this tenant gives a valid run row ...
    let (run_id, _) = t.export(&server).await;
    // ... whose archive file is replaced by one whose manifest totals 60 MB.
    let manifest = json!({
        "version": 2,
        "tools": [],
        "entries": [{"path": "tables/big/rows.jsonl", "sha256": "0".repeat(64), "bytes": 60 * 1024 * 1024}],
        "total_bytes": 60 * 1024 * 1024,
    });
    let archive = tar_gz(&[("manifest.json", serde_json::to_vec(&manifest).expect("json"))]);
    let path = server.state.db.data_dir().join("exports").join(format!("{run_id}.tar.gz"));
    std::fs::write(&path, archive).expect("replace archive");

    let err = t
        .client
        .tools_call("host.import", json!({"run_id": run_id}))
        .await
        .expect_err("60 MB must exceed the free plan's export_bytes_max");
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"]["name"], json!("export_bytes_max"), "{:?}", err.data);
    assert_eq!(err.data["plan"], json!("free"), "{:?}", err.data);
    assert_eq!(t.call("host.table.list", json!({})).await["tables"], json!([]), "no table may exist");
}

#[tokio::test]
async fn a_1000_row_table_and_10_docs_import_within_30s_with_rising_progress() {
    let server = TestServer::start().await;
    let a = timp::new_tenant(&server, "Timp AC6 Source").await;
    a.seed_table(1000).await;
    for i in 0..10 {
        a.seed_doc(&format!("doc{i}.md"), &format!("# doc {i}\n\nbody of document {i}")).await;
    }
    let (run_id, _) = a.export(&server).await;

    let b = timp::new_tenant(&server, "Timp AC6 Dest").await;
    let enq = b.call("host.import", json!({"run_id": run_id})).await;
    let import_id = enq["run_id"].as_str().expect("run_id").to_string();
    let mut samples: Vec<i64> = Vec::new();
    // The 30 s bound of the AC: a timeout, not a measured duration.
    let done: Value = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let got = b.call("host.runs.get", json!({"run_id": import_id})).await;
            samples.push(got["entries_done"].as_i64().unwrap_or(0));
            if got["status"] != json!("running") {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("import still running after 30 s");
    assert_eq!(done["status"], json!("done"), "{done}");
    println!("entries_done samples: {samples:?}");
    assert!(samples.windows(2).all(|w| w[0] <= w[1]), "entries_done must never fall: {samples:?}");
    let total = done["entries_total"].as_i64().expect("entries_total");
    assert!(total >= 10 + 10 + 3, "tables + docs + model entries: {done}");
    assert_eq!(done["entries_done"].as_i64(), Some(total), "{done}");
    assert_eq!(samples.last().copied(), Some(total));
    assert_eq!(done["result"]["imported"]["rows"], json!(1000), "{done}");
    assert_eq!(done["result"]["imported"]["docs"], json!(10), "{done}");
    assert_eq!(b.call("host.table.list", json!({})).await["tables"][0]["rows"], json!(1000));
}
