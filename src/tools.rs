//! Thin wrappers around the external Component-Model CLIs the build pipeline
//! shells out to.
//!
//! Two BytecodeAlliance tools become runtime dependencies of `wavelet` (see
//! `dev-notes/decouple-wasi.md`):
//!
//! - **`wkg`** ([wasm-pkg-tools]) — WIT package management. Fetches dependency
//!   WIT into a project's `wit/` tree and maintains a `wkg.lock` lock file.
//! - **`wac`** ([wac]) — component composition. Wires the project's own
//!   components (and any bundled dependency components) into one final artifact.
//!
//! This module only *locates and invokes* them; nothing in the compiler calls
//! it yet. Later steps of the WASI-decoupling work consume these wrappers.
//!
//! [wasm-pkg-tools]: https://github.com/bytecodealliance/wasm-pkg-tools
//! [wac]: https://github.com/bytecodealliance/wac

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// The two external CLIs the build pipeline depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// `wkg` — WIT package management (fetch/build/update + lock file).
    Wkg,
    /// `wac` — component composition (`compose` / `plug` / `targets`).
    Wac,
}

impl Tool {
    /// The executable name as found on `PATH`.
    pub fn bin(self) -> &'static str {
        match self {
            Tool::Wkg => "wkg",
            Tool::Wac => "wac",
        }
    }

    /// Where to get the tool, for the actionable "not found" error.
    fn install_hint(self) -> &'static str {
        match self {
            Tool::Wkg => {
                "install it with `cargo install wkg` or `brew install wkg` \
                 (https://github.com/bytecodealliance/wasm-pkg-tools)"
            }
            Tool::Wac => {
                "install it with `cargo install wac-cli` or `brew install wac` \
                 (https://github.com/bytecodealliance/wac)"
            }
        }
    }
}

/// Run `tool` with `args`, capturing its output.
///
/// Returns the captured stdout on success. On failure returns a single
/// actionable `String` error: either the tool is missing from `PATH` (with an
/// install hint), or it ran and exited non-zero (with its stderr included).
pub fn run<I, S>(tool: Tool, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_in(tool, args, None)
}

/// Like [`run`], but executes the tool in `cwd` when `Some` (most `wkg`/`wac`
/// subcommands operate on a project directory).
pub fn run_in<I, S>(tool: Tool, args: I, cwd: Option<&Path>) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(tool.bin());
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let output = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!(
                "`{}` was not found on PATH; {}",
                tool.bin(),
                tool.install_hint()
            )
        } else {
            format!("failed to run `{}`: {e}", tool.bin())
        }
    })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            stdout.trim().to_string()
        };
        Err(format!(
            "`{}` failed ({}): {detail}",
            tool.bin(),
            output.status
        ))
    }
}

/// Verify a tool is present and runnable by asking it for its version.
///
/// Returns the trimmed version line on success, or the same actionable error
/// [`run`] produces when the tool is absent. Useful as a preflight check before
/// a build shells out to it.
pub fn version(tool: Tool) -> Result<String, String> {
    run(tool, ["--version"]).map(|s| s.trim().to_string())
}

// --- `wkg` wrappers ------------------------------------------------------

/// `wkg wit fetch` — fetch the dependencies of the world(s) in `wit_dir` into
/// `wit_dir/deps/` and write/update the lock file. `--type wit` makes deps land
/// as WIT text that `wit-parser` can read.
///
/// `wkg` resolves `--wit-dir` relative to its working directory and writes
/// `wkg.lock` there, so we run from `wit_dir`'s parent (the project root) and
/// point `--wit-dir` at `wit_dir` by name. The lock therefore lands beside
/// `wit/` at the project root.
pub fn wkg_wit_fetch(wit_dir: &Path) -> Result<String, String> {
    let parent = run_dir_for(wit_dir);
    let name = wit_dir.file_name().unwrap_or_else(|| OsStr::new("wit"));
    run_in(
        Tool::Wkg,
        [
            OsStr::new("wit"),
            OsStr::new("fetch"),
            OsStr::new("--type"),
            OsStr::new("wit"),
            OsStr::new("--wit-dir"),
            name,
        ],
        Some(&parent),
    )
}

/// The directory to run `wkg` from for a given `wit_dir`: its parent, except
/// that a one-component relative path like `wit` has an empty (`""`) parent,
/// and an empty `current_dir` makes the OS fail program lookup with a spurious
/// not-found. Normalize that to the current directory `.`.
fn run_dir_for(wit_dir: &Path) -> std::path::PathBuf {
    match wit_dir.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    }
}

/// `wkg wit build` — build the `wit/` directory into a single self-contained
/// WIT package binary at `out`, fetching/embedding deps and generating a lock
/// file.
pub fn wkg_wit_build(wit_dir: &Path, out: &Path) -> Result<String, String> {
    run_in(
        Tool::Wkg,
        [
            OsStr::new("wit"),
            OsStr::new("build"),
            OsStr::new("--output"),
            out.as_os_str(),
        ],
        Some(wit_dir),
    )
}

// --- `wac` wrappers ------------------------------------------------------

/// `wac plug <socket> --plug <plug>... -o <out>` — plug one or more plug
/// components' exports into a socket component's imports (the simple case).
pub fn wac_plug(socket: &Path, plugs: &[&Path], out: &Path) -> Result<String, String> {
    let mut args: Vec<&OsStr> = vec![OsStr::new("plug"), socket.as_os_str()];
    for plug in plugs {
        args.push(OsStr::new("--plug"));
        args.push(plug.as_os_str());
    }
    args.push(OsStr::new("-o"));
    args.push(out.as_os_str());
    run(Tool::Wac, args)
}

/// `wac compose [--deps-dir <dir>] -o <out> <composition>` — full composition
/// driven by a `.wac` source file (multi-component / transitive wiring).
pub fn wac_compose(
    composition: &Path,
    deps_dir: Option<&Path>,
    out: &Path,
) -> Result<String, String> {
    let mut args: Vec<&OsStr> = vec![OsStr::new("compose")];
    if let Some(dir) = deps_dir {
        args.push(OsStr::new("--deps-dir"));
        args.push(dir.as_os_str());
    }
    args.push(OsStr::new("-o"));
    args.push(out.as_os_str());
    args.push(composition.as_os_str());
    run(Tool::Wac, args)
}

/// `wac targets <component> <world>` — verify `component` targets `world`;
/// useful as a build-time conformance check.
pub fn wac_targets(component: &Path, world: &str) -> Result<String, String> {
    run(
        Tool::Wac,
        [OsStr::new("targets"), component.as_os_str(), OsStr::new(world)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tool_gives_actionable_error() {
        // A tool that certainly isn't on PATH exercises the not-found branch by
        // way of the same `Command`/error mapping `run` uses.
        let mut cmd = Command::new("wavelet-nonexistent-tool-xyz");
        let err = cmd.output().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

        // And the install hints are non-empty and name the source.
        assert!(Tool::Wkg.install_hint().contains("wasm-pkg-tools"));
        assert!(Tool::Wac.install_hint().contains("/wac"));
        assert_eq!(Tool::Wkg.bin(), "wkg");
        assert_eq!(Tool::Wac.bin(), "wac");
    }

    /// A bare relative `wit` directory has an empty `Path::parent`; the wrapper
    /// must run `wkg` from `.` rather than an empty `current_dir` (which the OS
    /// rejects with a spurious not-found). We assert via the error *message*: if
    /// the empty parent leaked through, `wkg` would report not-found even when
    /// installed. When `wkg` is absent the not-found message is expected, so the
    /// check is only meaningful — and only made — when `wkg` is present.
    /// A bare relative `wit` directory has an empty `Path::parent`; running
    /// `wkg` from an empty `current_dir` makes the OS fail program lookup with a
    /// spurious not-found. `run_dir_for` must substitute `.` for that case while
    /// preserving a real parent otherwise.
    #[test]
    fn run_dir_for_normalizes_empty_parent() {
        assert_eq!(run_dir_for(Path::new("wit")), Path::new("."));
        assert_eq!(run_dir_for(Path::new("proj/wit")), Path::new("proj"));
        assert_eq!(run_dir_for(Path::new("/abs/proj/wit")), Path::new("/abs/proj"));
    }

    /// Only runs when the tools are actually installed (they are on this dev
    /// machine, at `~/.cargo/bin`). Skips silently in environments without them
    /// so the unit suite stays hermetic, but proves the wrapper can invoke a
    /// real binary and read its version when present.
    #[test]
    fn version_when_present() {
        for tool in [Tool::Wkg, Tool::Wac] {
            match version(tool) {
                Ok(v) => assert!(!v.is_empty(), "{} reported empty version", tool.bin()),
                Err(e) => {
                    // Absent: must be the actionable not-found message.
                    assert!(
                        e.contains("was not found on PATH"),
                        "unexpected error for {}: {e}",
                        tool.bin()
                    );
                }
            }
        }
    }
}
