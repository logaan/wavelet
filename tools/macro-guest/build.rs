//! Embed the macro-library source into the guest at build time.
//!
//! `wavelet build` sets `WAVELET_MACRO_SRC` to the absolute path of the `.wvl`
//! macro file being compiled into a component. We copy its contents into
//! `$OUT_DIR/macro_source.wvl`, which `src/lib.rs` pulls in with `include_str!`.
//! When the env var is unset (e.g. a bare `cargo build` of this crate for a
//! syntax check), we fall back to an empty library so the crate still compiles —
//! it just publishes no macros.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=WAVELET_MACRO_SRC");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let dest = Path::new(&out_dir).join("macro_source.wvl");

    let source = match std::env::var("WAVELET_MACRO_SRC") {
        Ok(path) => {
            println!("cargo:rerun-if-changed={path}");
            std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("WAVELET_MACRO_SRC `{path}`: {e}"))
        }
        // No macro file pinned: an empty library (compiles, publishes nothing).
        Err(_) => String::from("Package \"wavelet:empty-macros@0.1.0\"\n"),
    };

    std::fs::write(&dest, source)
        .unwrap_or_else(|e| panic!("writing {}: {e}", dest.display()));
}
