use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--strict` (3.9): Type::Unknown is a compile error rather than a gradual
    // top. Opt-in while the burn-down runs; accepted plan flips it to the
    // default (and deletes gradual mode) once the example + conformance suites
    // pass under it.
    if let Some(i) = args.iter().position(|a| a == "--strict") {
        args.remove(i);
        wavelet::check::set_strict(true);
    }
    match args.as_slice() {
        [cmd] if cmd == "--version" || cmd == "-V" || cmd == "version" => {
            println!("wavelet {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        [cmd] if cmd == "read" => read_stdin_cmd(),
        [cmd, path] if cmd == "read" => read_cmd(path),
        [cmd, path] if cmd == "expand" => expand_cmd(path),
        [cmd] if cmd == "repl" => match wavelet::repl::repl() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        [cmd, path] if cmd == "wit" => wit_cmd(path),
        [cmd, rest @ ..] if cmd == "new" && !rest.is_empty() => new_cmd(rest),
        [cmd, rest @ ..] if cmd == "run" && !rest.is_empty() => run_cmd(rest),
        [cmd, rest @ ..] if cmd == "build" && !rest.is_empty() => build_cmd(rest),
        [cmd, rest @ ..] if cmd == "compose" && !rest.is_empty() => compose_cmd(rest),
        // Bare `wavelet <file> [args...]` runs the script directly — the
        // invocation a `#!/usr/bin/env wavelet` shebang produces for an
        // executable script (7.2), which is commonly extensionless. Any trailing
        // arguments are the script's own; `run` ignores them today, exactly as
        // `wavelet run` does. `is_script_path` requires an existing, non-
        // subcommand file, so this can't shadow the bare-word subcommands above.
        [first, ..] if is_script_path(first) => run_cmd(&args[..1]),
        _ => {
            eprintln!("usage: wavelet read [file.wlt]");
            eprintln!("       wavelet expand <file.wlt>");
            eprintln!("       wavelet repl");
            eprintln!("       wavelet wit <file.wlt>");
            eprintln!("       wavelet new <name> [--type=cli|http]");
            eprintln!("       wavelet run <file.wlt>... [-- <args>...]");
            eprintln!("       wavelet build <file.wlt>... [-o <dir>]");
            eprintln!("       wavelet compose <entry.wasm> <plug.wasm>... [-o <app.wasm>]");
            eprintln!("       wavelet <file> [args...]       (run a script; also via #! shebang)");
            eprintln!("       wavelet --version");
            eprintln!("options: --strict   (typecheck with Unknown as an error)");
            ExitCode::from(2)
        }
    }
}

/// Project root for a source file: the parent of the `src/` dir it lives in
/// (so foreign macro imports resolve their `.wasm` the same way `wavelet
/// build` does); `.` when there is no parent.
/// The bare-word subcommands `wavelet` dispatches on. A first CLI argument that
/// is one of these is a command, never a script to run — this keeps
/// `is_script_path` from stealing e.g. `wavelet repl` even if a file named
/// `repl` happens to sit in the working directory.
const KNOWN_SUBCOMMANDS: &[&str] = &[
    "version", "read", "expand", "repl", "wit", "new", "run", "build", "compose",
];

/// Whether a first CLI argument should be taken as a script path to run rather
/// than a subcommand. This is what backs shebang execution (`#!/usr/bin/env
/// wavelet`): for an *executable* script the kernel invokes `wavelet
/// /path/to/script [script-args...]`, and Unix scripts are commonly
/// extensionless (an executable named `deploy`, no suffix), so matching only
/// `.wlt` would silently drop them.
///
/// An argument is a script when it names an existing file *and* is not one of
/// the bare-word subcommands. `.wlt` stays a fast-path so a `wavelet
/// missing.wlt` still reaches `run` (and its "cannot read" error) rather than
/// the usage message.
fn is_script_path(arg: &str) -> bool {
    arg.ends_with(".wlt")
        || (!KNOWN_SUBCOMMANDS.contains(&arg) && std::path::Path::new(arg).is_file())
}

fn project_root_of(path: &str) -> std::path::PathBuf {
    std::path::Path::new(path)
        .parent()
        .and_then(|d| d.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

fn wit_cmd(path: &str) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Expand macros (`Derive`, foreign macros) before synthesizing WIT, using the
    // same foreign-macro-aware pipeline `wavelet build`/`wavelet expand` use, so
    // `wavelet wit` agrees with `wavelet build` about the same source. Project
    // root = parent of the `src/` dir the file lives in; default to `.` when
    // there is no parent.
    let root = project_root_of(path);
    let result = wavelet::macrodep::read_file_with_macros(&src, &root)
        .map_err(|e| e.to_string())
        .and_then(|(arena, roots)| {
            let mut foreign = wavelet::macrodep::FileExpander::for_file(&root, &arena, &roots);
            wavelet::expand::expand_file(
                arena,
                &roots,
                foreign
                    .as_mut()
                    .map(|f| f as &mut dyn wavelet::expand::ForeignExpander),
            )
        })
        .and_then(|(arena, roots)| {
            // Type-check the expanded program before synthesizing its WIT, so an
            // ill-typed program is a `wit` error too — the static guarantee holds
            // on this path as well, not only the playground.
            wavelet::check::check_program(&arena, &roots)?;
            wavelet::wit::synthesize(&arena, &roots)
        });
    match result {
        Ok(wit) => {
            print!("{wit}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            ExitCode::FAILURE
        }
    }
}

fn split_out(rest: &[String], default: &str) -> (Vec<String>, String) {
    match rest.iter().position(|a| a == "-o") {
        Some(i) if i + 1 < rest.len() => (rest[..i].to_vec(), rest[i + 1].clone()),
        _ => (rest.to_vec(), default.to_string()),
    }
}

fn build_cmd(rest: &[String]) -> ExitCode {
    let (files, out_dir) = split_out(rest, "out");
    match wavelet::build::build_files(&files, &out_dir) {
        Ok(outputs) => {
            for o in outputs {
                println!("{o}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn compose_cmd(rest: &[String]) -> ExitCode {
    let (files, out) = split_out(rest, "app.wasm");
    if files.is_empty() {
        eprintln!("compose: no input components");
        return ExitCode::from(2);
    }
    match wavelet::build::compose_files(&files, &out) {
        Ok(()) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn new_cmd(rest: &[String]) -> ExitCode {
    use wavelet::scaffold::{self, ProjectKind};

    let mut name: Option<&str> = None;
    let mut kind_str: Option<&str> = None;
    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        if let Some(v) = arg.strip_prefix("--type=") {
            kind_str = Some(v);
        } else if arg == "--type" || arg == "-t" {
            match rest.get(i + 1) {
                Some(v) => {
                    kind_str = Some(v);
                    i += 1;
                }
                None => {
                    eprintln!("new: `{arg}` needs a value (e.g. --type=http)");
                    return ExitCode::from(2);
                }
            }
        } else if arg.starts_with('-') {
            eprintln!("new: unknown option `{arg}`");
            return ExitCode::from(2);
        } else if name.is_none() {
            name = Some(arg);
        } else {
            eprintln!("new: unexpected argument `{arg}`");
            return ExitCode::from(2);
        }
        i += 1;
    }

    let name = match name {
        Some(n) => n,
        None => {
            eprintln!("new: missing project name (usage: wavelet new <name> [--type=http])");
            return ExitCode::from(2);
        }
    };

    // `cli` is the default when `--type` is omitted.
    let kind = match kind_str {
        Some(s) => match ProjectKind::parse(s) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("new: {e}");
                return ExitCode::from(2);
            }
        },
        None => ProjectKind::default(),
    };

    match scaffold::create(name, kind) {
        Ok((root, files)) => {
            println!("created {}/", root.display());
            for f in &files {
                println!("  {}", f.display());
            }

            // Vendor the new project's dependency WIT into `wit/deps` and write
            // `wkg.lock`, so a fresh project's deps are pinned. Best-effort: a
            // missing `wkg` or no network just leaves `wit/` unfetched.
            let srcs: Vec<std::path::PathBuf> = files
                .iter()
                .filter(|f| f.extension().and_then(|e| e.to_str()) == Some("wlt"))
                .cloned()
                .collect();
            if let Err(e) = wavelet::build::populate_project_wit(&root, &srcs) {
                eprintln!("warning: could not fetch dependency WIT via wkg: {e}");
            }

            let start = match kind {
                ProjectKind::Cli => "scripts/run.sh",
                ProjectKind::Http => "scripts/serve.sh",
            };
            println!("\nnext:\n  cd {}\n  {start}", root.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("new: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_cmd(rest: &[String]) -> ExitCode {
    // Everything before a `--` separator is treated as files; anything after is
    // ignored (the removed `args` builtin used to read it). Keep accepting the
    // `--` form so existing invocations don't error.
    let files: Vec<String> = match rest.iter().position(|a| a == "--") {
        Some(i) => rest[..i].to_vec(),
        None => rest.to_vec(),
    };
    match wavelet::runner::run(&files) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn expand_cmd(path: &str) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Project root = parent of the `src/` dir the file lives in, so foreign
    // macro imports (`Import {… macros: true}`) resolve their `.wasm` the same
    // way `wavelet build` does. Default to `.` when there is no parent.
    let root = project_root_of(path);
    let result = wavelet::macrodep::read_file_with_macros(&src, &root)
        .map_err(|e| e.to_string())
        .and_then(|(arena, roots)| {
            let mut foreign = wavelet::macrodep::FileExpander::for_file(&root, &arena, &roots);
            wavelet::expand::expand_file(
                arena,
                &roots,
                foreign
                    .as_mut()
                    .map(|f| f as &mut dyn wavelet::expand::ForeignExpander),
            )
        });
    match result {
        Ok((arena, roots)) => {
            for root in roots {
                println!("{}", wavelet::print(&arena, root));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read_cmd(path: &str) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    read_source(&src, path)
}

fn read_stdin_cmd() -> ExitCode {
    use std::io::Read;
    let mut src = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut src) {
        eprintln!("error: cannot read <stdin>: {e}");
        return ExitCode::FAILURE;
    }
    read_source(&src, "<stdin>")
}

fn read_source(src: &str, label: &str) -> ExitCode {
    match wavelet::read_file(src) {
        Ok((arena, roots)) => {
            for root in roots {
                println!("{}", wavelet::print(&arena, root));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{label}: {e}");
            ExitCode::FAILURE
        }
    }
}
