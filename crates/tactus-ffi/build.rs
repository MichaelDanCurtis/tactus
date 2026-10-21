//! Generate `include/tactus.h` from the Rust source, so the C header can never
//! silently drift from the ABI it documents.

fn main() {
    let crate_dir = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(d) => d,
        Err(_) => return,
    };
    let out = std::path::Path::new(&crate_dir)
        .join("include")
        .join("tactus.h");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let config = cbindgen::Config::from_root_or_default(&crate_dir);
    // A header-generation failure must not break `cargo build`: the library is
    // still valid, and a stale header is a clearer failure than no build.
    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
    {
        Ok(b) => {
            b.write_to_file(&out);
        }
        Err(e) => println!("cargo:warning=cbindgen could not generate tactus.h: {e}"),
    }
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/device.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
