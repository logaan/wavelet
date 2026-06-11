use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, path] if cmd == "read" => read_cmd(path),
        _ => {
            eprintln!("usage: wavelet read <file.wvl>");
            eprintln!("  parse a Wavelet file and print each form as canonical WAVE");
            ExitCode::from(2)
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
