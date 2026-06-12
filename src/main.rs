use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
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
        [cmd, rest @ ..] if cmd == "run" && !rest.is_empty() => run_cmd(rest),
        [cmd, rest @ ..] if cmd == "build" && !rest.is_empty() => build_cmd(rest),
        [cmd, rest @ ..] if cmd == "compose" && !rest.is_empty() => compose_cmd(rest),
        _ => {
            eprintln!("usage: wavelet read <file.wvl>");
            eprintln!("       wavelet expand <file.wvl>");
            eprintln!("       wavelet repl");
            eprintln!("       wavelet wit <file.wvl>");
            eprintln!("       wavelet run <file.wvl>... [-- <args>...]");
            eprintln!("       wavelet build <file.wvl>... [-o <dir>]");
            eprintln!("       wavelet compose <entry.wasm> <plug.wasm>... [-o <app.wasm>]");
            ExitCode::from(2)
        }
    }
}

fn wit_cmd(path: &str) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    match wavelet::read_file(&src).map_err(|e| e.to_string()).and_then(|(arena, roots)| {
        wavelet::wit::synthesize(&arena, &roots)
    }) {
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

fn split_out<'a>(rest: &'a [String], default: &str) -> (Vec<String>, String) {
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

fn run_cmd(rest: &[String]) -> ExitCode {
    let (files, prog_args) = match rest.iter().position(|a| a == "--") {
        Some(i) => (rest[..i].to_vec(), rest[i + 1..].to_vec()),
        None => (rest.to_vec(), vec![]),
    };
    match wavelet::runner::run_files(&files, prog_args) {
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
    match wavelet::read_file(&src)
        .map_err(|e| e.to_string())
        .and_then(|(arena, roots)| wavelet::expand::expand_file(arena, &roots))
    {
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
    match wavelet::read_file(&src) {
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
