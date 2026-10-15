#![allow(dead_code)]

use std::path::PathBuf;

use serde_json::Value;

pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn read(path: &str) -> String {
    let full = crate_dir().join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {}: {e}", full.display()))
}

pub fn fixture(name: &str) -> Value {
    serde_json::from_str(&read(&format!("tests/fixtures/{name}"))).expect("fixture is JSON")
}

pub fn surface() -> Value {
    serde_json::from_str(&read("knowledge/appport-surface.json")).expect("surface is JSON")
}

/// Compares `actual` with `tests/snapshots/<name>`. Set `UPDATE_SNAPSHOTS=1` to rewrite.
pub fn snapshot(name: &str, actual: &str) {
    let path = crate_dir().join("tests/snapshots").join(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing snapshot {}; run with UPDATE_SNAPSHOTS=1", path.display()));
    assert_eq!(expected, actual, "snapshot {name} changed; review and run with UPDATE_SNAPSHOTS=1 to accept");
}
