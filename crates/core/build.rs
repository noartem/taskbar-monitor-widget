//! Turns `strings.tsv` (key<TAB>English<TAB>Russian) into a sorted static table for `i18n.rs`.

use std::collections::BTreeMap;
use std::fmt::Write;

fn main() {
    println!("cargo:rerun-if-changed=strings.tsv");
    let text = std::fs::read_to_string("strings.tsv").expect("strings.tsv");
    let mut map = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        assert!(parts.len() == 3, "strings.tsv:{}: expected 3 tab-separated columns", n + 1);
        let unescape = |s: &str| s.replace("\\n", "\n");
        if map.insert(parts[0].to_string(), (unescape(parts[1]), unescape(parts[2]))).is_some() {
            panic!("strings.tsv:{}: duplicate key {}", n + 1, parts[0]);
        }
    }
    let mut out = String::from("&[\n");
    for (k, (en, ru)) in &map {
        writeln!(out, "    ({k:?}, {en:?}, {ru:?}),").unwrap();
    }
    out.push(']');
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("strings.rs");
    std::fs::write(path, out).unwrap();
}
