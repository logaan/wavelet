use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, path] if cmd == "read" => read_cmd(path),
        [cmd, path] if cmd == "wit" => wit_cmd(path),
        [cmd, rest @ ..] if cmd == "run" && !rest.is_empty() => run_cmd(rest),
        _ => {
            eprintln!("usage: wavelet read <file.wvl>");
            eprintln!("       wavelet wit <file.wvl>");
            eprintln!("       wavelet run <file.wvl>... [-- <args>...]");
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
