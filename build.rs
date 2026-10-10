fn main() {
    // with_crate parses the root module tree and uses Cargo metadata. Keep
    // generated headers and unrelated tests/docs out of the input watch set.
    for input in ["cbindgen.toml", "Cargo.toml", "Cargo.lock", "src"] {
        println!("cargo::rerun-if-changed={input}");
    }
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR");
    let include_dir = std::path::Path::new(&crate_dir).join("include");
    std::fs::create_dir_all(&include_dir).expect("failed to create include directory");

    let config = cbindgen::Config::from_file("cbindgen.toml").unwrap_or_default();
    cbindgen::Builder::new()
        .with_crate(crate_dir)
        .with_config(config)
        .generate()
        .expect("failed to generate C header")
        .write_to_file(include_dir.join("omoikane.h"));
}
