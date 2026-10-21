//! The capability declaration is fixed inside the module (the host reads it without running
//! the module), so the panel host, the claimable hostnames and the backend networks must be
//! known at build time. `WAYHOUSE_PELICAN_CAPS=/path/to/caps.json cargo build ...` bakes in
//! the operator's own declaration; without it the placeholder `caps.json` is used.
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=WAYHOUSE_PELICAN_CAPS");
    println!("cargo:rerun-if-changed=caps.json");
    let src = env::var("WAYHOUSE_PELICAN_CAPS").unwrap_or_else(|_| "caps.json".into());
    println!("cargo:rerun-if-changed={src}");
    let text = fs::read_to_string(&src).unwrap_or_else(|e| panic!("reading {src}: {e}"));
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("caps.json");
    fs::write(out, text.trim()).unwrap();
}
