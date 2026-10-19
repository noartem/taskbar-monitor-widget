//! Turns `strings.tsv` (key, then one column per language in `i18n::Lang` order) into a sorted static table.

use std::collections::BTreeMap;
use std::fmt::Write;

/// Must match the number of `i18n::Lang` variants.
const LANGS: usize = 7;

fn main() {
    println!("cargo:rerun-if-changed=strings.tsv");
    let text = std::fs::read_to_string("strings.tsv").expect("strings.tsv");
    let mut map = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        assert!(parts.len() == LANGS + 1, "strings.tsv:{}: expected {} tab-separated columns", n + 1, LANGS + 1);
        let values: Vec<String> = parts[1..].iter().map(|s| s.replace("\\n", "\n")).collect();
        assert!(values.iter().all(|v| !v.is_empty()), "strings.tsv:{}: empty translation", n + 1);
        if map.insert(parts[0].to_string(), values).is_some() {
            panic!("strings.tsv:{}: duplicate key {}", n + 1, parts[0]);
        }
    }
    let mut out = String::from("&[\n");
    for (k, values) in &map {
        write!(out, "    ({k:?}, [").unwrap();
        for v in values {
            write!(out, "{v:?}, ").unwrap();
        }
        out.push_str("]),\n");
    }
    out.push(']');
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("strings.rs");
    std::fs::write(path, out).unwrap();
}
