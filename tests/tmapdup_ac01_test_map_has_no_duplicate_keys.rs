//! agent/test-map.json has no duplicated JSON object key at any depth
//! 2026-10-18: `ac_test_map_by_prefix` carried `"orgmb"` twice. JSON tools (serde_json,
//! jq, python) keep only the LAST copy without a word, so any merge of the file
//! silently drops data. serde_json cannot report this itself, so the scan below is a
//! `DeserializeSeed` visitor that watches every map's keys and names each repeated
//! key by its path (`/ac_test_map_by_prefix/orgmb`).

use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

struct Scan<'a> {
    path: String,
    dups: &'a RefCell<Vec<String>>,
}

impl<'de, 'a> DeserializeSeed<'de> for Scan<'a> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de, 'a> Visitor<'de> for Scan<'a> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            let path = format!("{}/{}", self.path, key);
            if !seen.insert(key) {
                self.dups.borrow_mut().push(path.clone());
            }
            map.next_value_seed(Scan { path, dups: self.dups })?;
        }
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let mut i = 0usize;
        while seq
            .next_element_seed(Scan { path: format!("{}[{i}]", self.path), dups: self.dups })?
            .is_some()
        {
            i += 1;
        }
        Ok(())
    }

    fn visit_bool<E>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }
}

/// Every duplicated key path in `json`, in document order (a key repeated N times is
/// named N-1 times).
fn duplicate_keys(json: &str) -> Vec<String> {
    let dups = RefCell::new(Vec::new());
    let mut de = serde_json::Deserializer::from_str(json);
    Scan { path: String::new(), dups: &dups }
        .deserialize(&mut de)
        .and_then(|()| de.end())
        .unwrap_or_else(|e| panic!("not valid JSON: {e}"));
    dups.into_inner()
}

#[test]
fn the_scan_names_duplicates_at_any_depth() {
    assert!(duplicate_keys(r#"{"a":{"b":1},"c":[{"d":1},{"d":2}]}"#).is_empty());
    assert_eq!(duplicate_keys(r#"{"a":1,"a":2}"#), ["/a"]);
    assert_eq!(
        duplicate_keys(r#"{"m":{"x":{"k":1,"k":2}},"l":[{"z":1,"z":1}],"m2":1}"#),
        ["/m/x/k", "/l[0]/z"]
    );
}

#[test]
fn agent_test_map_json_has_no_duplicate_keys() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("agent/test-map.json");
    let text = std::fs::read_to_string(&path).expect("read agent/test-map.json");
    let dups = duplicate_keys(&text);
    assert!(
        dups.is_empty(),
        "agent/test-map.json repeats these keys (JSON tools keep only the last copy, so a merge silently loses data): {dups:?}"
    );
}
