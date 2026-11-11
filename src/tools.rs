use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Wkg,

    Wac,
}

impl Tool {
    pub fn bin(self) -> &'static str {
        match self {
            Tool::Wkg => "wkg",
            Tool::Wac => "wac",
        }
    }

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

pub fn run<I, S>(tool: Tool, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_in(tool, args, None)
}

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

pub fn version(tool: Tool) -> Result<String, String> {
    run(tool, ["--version"]).map(|s| s.trim().to_string())
}

pub fn wkg_wit_fetch(wit_dir: &Path) -> Result<String, String> {
    wkg_wit_fetch_from(wit_dir, &run_dir_for(wit_dir))
}

pub(crate) fn wkg_wit_fetch_from(wit_dir: &Path, project_root: &Path) -> Result<String, String> {
    let directory = wit_dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", wit_dir.display()))?;
    run_in(
        Tool::Wkg,
        [
            OsStr::new("wit"),
            OsStr::new("fetch"),
            OsStr::new("--type"),
            OsStr::new("wit"),
            directory.as_os_str(),
        ],
        Some(project_root),
    )
}

fn run_dir_for(wit_dir: &Path) -> std::path::PathBuf {
    match wit_dir.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    }
}

pub fn wkg_wit_build(wit_dir: &Path, out: &Path) -> Result<String, String> {
    run_in(
        Tool::Wkg,
        [
            OsStr::new("wit"),
            OsStr::new("build"),
            OsStr::new("--wit-dir"),
            OsStr::new("."),
            OsStr::new("--output"),
            out.as_os_str(),
        ],
        Some(wit_dir),
    )
}

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

pub fn wac_compose(
    composition: &Path,
    deps_dir: Option<&Path>,
    deps: &[(String, &Path)],
    out: &Path,
) -> Result<String, String> {
    let dep_args: Vec<std::ffi::OsString> = deps
        .iter()
        .map(|(name, path)| {
            let mut s = std::ffi::OsString::from(name);
            s.push("=");
            s.push(path.as_os_str());
            s
        })
        .collect();

    let mut args: Vec<&OsStr> = vec![OsStr::new("compose")];
    if let Some(dir) = deps_dir {
        args.push(OsStr::new("--deps-dir"));
        args.push(dir.as_os_str());
    }
    for d in &dep_args {
        args.push(OsStr::new("-d"));
        args.push(d.as_os_str());
    }
    args.push(OsStr::new("-o"));
    args.push(out.as_os_str());
    args.push(composition.as_os_str());
    run(Tool::Wac, args)
}

pub fn wac_targets(component: &Path, world: &str) -> Result<String, String> {
    run(
        Tool::Wac,
        [
            OsStr::new("targets"),
            component.as_os_str(),
            OsStr::new(world),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tool_gives_actionable_error() {
        let mut cmd = Command::new("wavelet-nonexistent-tool-xyz");
        let err = cmd.output().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

        assert!(Tool::Wkg.install_hint().contains("wasm-pkg-tools"));
        assert!(Tool::Wac.install_hint().contains("/wac"));
        assert_eq!(Tool::Wkg.bin(), "wkg");
        assert_eq!(Tool::Wac.bin(), "wac");
    }

    #[test]
    fn run_dir_for_normalizes_empty_parent() {
        assert_eq!(run_dir_for(Path::new("wit")), Path::new("."));
        assert_eq!(run_dir_for(Path::new("proj/wit")), Path::new("proj"));
        assert_eq!(
            run_dir_for(Path::new("/abs/proj/wit")),
            Path::new("/abs/proj")
        );
    }

    #[test]
    fn version_when_present() {
        for tool in [Tool::Wkg, Tool::Wac] {
            match version(tool) {
                Ok(v) => assert!(!v.is_empty(), "{} reported empty version", tool.bin()),
                Err(e) => {
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
