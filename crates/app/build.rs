//! Embeds the icon, manifest (DPI awareness, common controls v6, Windows 10+ compatibility) and version info.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=resources/app.rc.in");
    println!("cargo:rerun-if-changed=resources/app.manifest");
    println!("cargo:rerun-if-changed=../../assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u16> = version.split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    parts.resize(4, 0);
    let numeric = parts.iter().map(u16::to_string).collect::<Vec<_>>().join(",");

    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = |p: &Path| p.canonicalize().unwrap().display().to_string().replace('\\', "/");
    let rc = std::fs::read_to_string(dir.join("resources/app.rc.in"))
        .unwrap()
        .replace("@ICON@", &path(&dir.join("../../assets/app.ico")))
        .replace("@MANIFEST@", &path(&dir.join("resources/app.manifest")))
        .replace("@VERSION_NUM@", &numeric)
        .replace("@VERSION@", &version);
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("app.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_required().unwrap();
}
