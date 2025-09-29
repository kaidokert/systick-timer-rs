use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

/* This overrides memory.x provided by feather_m0 crate */
fn main() {
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());

    // Create our memory.x
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(include_bytes!("memory.x"))
        .unwrap();

    // Tell cargo to use this directory for linking - this should come FIRST
    println!("cargo:rustc-link-search={}", out.display());

    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");

    // Link args
    println!("cargo:rustc-link-arg=-Tlink.x");
}
