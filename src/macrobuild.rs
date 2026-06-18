//! Produce a `wavelet:meta/macros` component from a Wavelet macro file
//! (design.md §6.3; Step 9, **strategy A: interpreter-in-a-component**).
//!
//! The payoff of the macro-component feature is that a macro library can be
//! *written in Wavelet itself*: a `.wvl` file whose top level is `DefMacro`s is
//! compiled into a component exporting `wavelet:meta/macros`, which the Step 1–8
//! consumer then imports with `macros: true` and uses exactly like a hand-built
//! macro component. Wavelet thereby dogfoods its own macro system.
//!
//! ## Strategy A: bundle the interpreter, carry the macros as data
//!
//! Rather than compile each macro *body* to a wasm function (strategy B, a large
//! `emit.rs` extension — deferred), the produced component bundles the Wavelet
//! interpreter — the semantics oracle (`CLAUDE.md`) — plus the macro file's
//! source as embedded data. Its `manifest`/`expand` exports run
//! [`crate::macrolib`] over that source. Because the *same* interpreter and the
//! *same* `expand_once` drive both this component and the local ahead-of-time
//! expander ([`crate::expand::expand_file`]), a macro means exactly the same
//! thing whether expanded locally or through the component.
//!
//! The mechanics: a small, checked-in guest crate (`tools/macro-guest`) depends
//! on the `wavelet` crate (so the interpreter is *in* the guest) and exports
//! `wavelet:meta/macros`. We point it at the macro file via the
//! `WAVELET_MACRO_SRC` env var (its `build.rs` embeds the source), build it for
//! `wasm32-unknown-unknown` (**no WASI** — the component must instantiate under
//! the consumer's empty, capability-free linker, Step 2), and componentize the
//! core module in-process with [`wit_component`] (the metadata wit-bindgen
//! embeds is enough — no `wasm-tools` shell-out).
//!
//! ## How a file declares it is a macro library
//!
//! A file is a macro library iff its top level is a `Package` declaration and
//! `DefMacro`s only — it declares **no `Export`** and defines no runtime
//! functions/values ([`is_macro_library`]). This needs no new syntax or lexer
//! token: "a file that only defines macros and exports none of its own runtime
//! funcs" (the step's suggested trigger). `wavelet build` routes such a file
//! here instead of through the ordinary [`crate::emit`] path.
//!
//! This module is native-only (it shells out to `cargo` and builds a sibling
//! crate), gated `#[cfg(not(target_arch = "wasm32"))]` in `lib.rs` alongside the
//! rest of the producer/consumer build machinery.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::form::{Arena, Node, NodeId};

/// Does this file declare itself a macro library?
///
/// True iff every top-level form is a `Package` declaration or a `DefMacro`,
/// there is at least one `DefMacro`, and there is no `Export` (nor any runtime
/// `Def`/`DefType`/`Import`). Such a file has no runtime surface of its own — its
/// whole purpose is to publish macros — so `wavelet build` compiles it into a
/// `wavelet:meta/macros` component rather than an ordinary component.
pub fn is_macro_library(arena: &Arena, roots: &[NodeId]) -> bool {
    let mut saw_macro = false;
    let mut saw_package = false;
    for &root in roots {
        let Node::Tup(items) = arena.node(root) else { return false };
        let Some(&head) = items.first() else { return false };
        let Node::Sym(head_name) = arena.node(head) else { return false };
        match head_name.as_str() {
            "package-MACRO" => saw_package = true,
            "defmacro-MACRO" => saw_macro = true,
            // Anything else — Export, Def, DefType, Import, a bare expression —
            // means the file has a runtime surface, so it is NOT a pure macro
            // library and takes the ordinary build path.
            _ => return false,
        }
    }
    saw_package && saw_macro
}

/// Build a macro-library `.wvl` source into a `wavelet:meta/macros` component,
/// returning the component bytes.
///
/// `src` is the macro file's text; `guest_crate` is the path to the
/// `tools/macro-guest` crate (see [`default_guest_crate`]). This:
///
/// 1. writes `src` to a temp file and points the guest at it via
///    `WAVELET_MACRO_SRC`;
/// 2. runs `cargo build --release --target wasm32-unknown-unknown` in the guest
///    crate (no WASI, capability-free);
/// 3. componentizes the resulting core module in-process.
///
/// Errors are actionable strings: a missing `wasm32` target, a `cargo` failure
/// (with its stderr), or a componentization failure.
pub fn build_macro_component(src: &str, guest_crate: &Path) -> Result<Vec<u8>, String> {
    // 1. Stage the macro source where the guest's build.rs can read it.
    let src_file = stage_source(src)?;

    // 2. Build the guest core module for wasm32 (no WASI).
    let core = build_guest_core(guest_crate, &src_file)?;

    // 3. Componentize in-process (wit-bindgen already embedded the metadata).
    componentize(&core)
}

/// The default location of the bundled guest crate: `tools/macro-guest` beside
/// the `wavelet` crate's manifest. `wavelet build` uses this so a user never has
/// to know the guest exists.
pub fn default_guest_crate() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tools")
        .join("macro-guest")
}

/// Write `src` to a uniquely-named temp file and return its path. The guest's
/// `build.rs` reads `WAVELET_MACRO_SRC` (set to this path) and embeds the
/// contents.
fn stage_source(src: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir();
    let file = dir.join(format!(
        "wavelet-macro-src-{}-{}.wvl",
        std::process::id(),
        // A monotonic-ish suffix so concurrent builds don't collide.
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&file, src).map_err(|e| format!("staging macro source: {e}"))?;
    Ok(file)
}

/// Run `cargo build --release --target wasm32-unknown-unknown` in the guest
/// crate with `WAVELET_MACRO_SRC` set, and return the built core-module bytes.
fn build_guest_core(guest_crate: &Path, src_file: &Path) -> Result<Vec<u8>, String> {
    let status = Command::new("cargo")
        .current_dir(guest_crate)
        .env("WAVELET_MACRO_SRC", src_file)
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .output()
        .map_err(|e| format!("running cargo for the macro guest: {e}"))?;
    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(format!(
            "building the macro-library component failed.\n\
             (the `wasm32-unknown-unknown` target must be installed: \
             `rustup target add wasm32-unknown-unknown`)\n--- cargo ---\n{}",
            stderr.trim()
        ));
    }
    let wasm = guest_crate
        .join("target")
        .join("wasm32-unknown-unknown")
        .join("release")
        .join("wavelet_macro_guest.wasm");
    std::fs::read(&wasm).map_err(|e| format!("reading guest core module {}: {e}", wasm.display()))
}

/// Wrap a core wasm module (already carrying wit-bindgen's embedded
/// component-type metadata) into a component, in-process, with
/// [`wit_component::ComponentEncoder`] — the same encoder [`crate::emit`] uses,
/// so no `wasm-tools` binary is required.
fn componentize(core: &[u8]) -> Result<Vec<u8>, String> {
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(core)
        .map_err(|e| format!("componentizing the macro library failed: {e:#}"))?
        .encode()
        .map_err(|e| format!("encoding the macro-library component failed: {e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::read_file;

    fn roots(src: &str) -> (Arena, Vec<NodeId>) {
        read_file(src).expect("read")
    }

    #[test]
    fn pure_macro_file_is_a_macro_library() {
        let (a, r) = roots(
            "Package \"demo:macros@0.1.0\"\n\
             DefMacro identity {x} x\n\
             DefMacro unless {cond body}\n  Quasi If Unquote(cond) {} Unquote(body)\n",
        );
        assert!(is_macro_library(&a, &r));
    }

    #[test]
    fn file_with_export_is_not_a_macro_library() {
        let (a, r) = roots(
            "Package \"demo:app@0.1.0\"\n\
             DefMacro identity {x} x\n\
             Export greet\n\
             Def greet Fn {} \"hi\"\n",
        );
        assert!(!is_macro_library(&a, &r));
    }

    #[test]
    fn file_with_no_macros_is_not_a_macro_library() {
        let (a, r) = roots("Package \"demo:app@0.1.0\"\n");
        assert!(!is_macro_library(&a, &r));
    }

    #[test]
    fn file_with_runtime_def_is_not_a_macro_library() {
        let (a, r) = roots(
            "Package \"demo:app@0.1.0\"\n\
             DefMacro identity {x} x\n\
             Def x 1\n",
        );
        assert!(!is_macro_library(&a, &r));
    }
}
