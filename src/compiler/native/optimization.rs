use std::process::Command;

pub(super) fn optimize(module: Vec<u8>) -> Result<Vec<u8>, String> {
    let directory =
        tempfile::tempdir().map_err(|e| format!("optimizer temporary directory: {e}"))?;
    let input = directory.path().join("input.wasm");
    let output = directory.path().join("output.wasm");
    std::fs::write(&input, module).map_err(|e| format!("optimizer input: {e}"))?;
    let result = Command::new("wasm-opt")
        .args(["-Oz", "--mvp-features", "--enable-mutable-globals", "--enable-sign-ext", "--enable-nontrapping-float-to-int", "--enable-bulk-memory", "--enable-tail-call", "--enable-reference-types", "--enable-multivalue"])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .map_err(|e| format!("size optimization requires `wasm-opt` on PATH; install Binaryen (https://github.com/WebAssembly/binaryen): {e}"))?;
    if !result.status.success() {
        return Err(format!(
            "wasm-opt failed ({}): {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    std::fs::read(output).map_err(|e| format!("optimizer output: {e}"))
}
