#[path = "build_support/revision.rs"]
mod revision;

fn main() {
    // cbindgen watches its Rust/config inputs; revision::emit separately
    // watches Git metadata and tracked files for the embedded dirty state.
    for input in [
        "cbindgen.toml",
        "Cargo.toml",
        "Cargo.lock",
        "src",
        "build_support",
    ] {
        println!("cargo::rerun-if-changed={input}");
    }
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR");
    revision::emit(std::path::Path::new(&crate_dir));
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
