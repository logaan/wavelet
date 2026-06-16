//! End-to-end composition tests (Step 12 of the WASI-decoupling plan).
//!
//! `wavelet build` now produces **one** final composed artifact: it emits each
//! component, generates a `.wac` wiring file, and runs `wac compose` (via the
//! Step 0 wrapper) into `<out>/app.wasm`, leaving host (`wasi:*`) imports
//! unsatisfied for the runtime. These tests scaffold the `cli` and `http`
//! templates, build them, and then actually **run** (`wasmtime run`) / **serve**
//! (`wasmtime serve` + a request) the composed component, asserting on real
//! output — not just template text. A third test composes a multi-component
//! (`demo:main` + `demo:shout`) project entirely from local interfaces and
//! checks it collapses to a single component.
//!
//! Because the build needs the real host WIT under `wit/deps` (fetched by `wkg`)
//! and the composed artifact is produced by `wac`, the template tests are gated
//! on `wkg`/`wac`/`wasmtime` being installed *and* a registry fetch succeeding —
//! they skip cleanly in toolless/offline CI rather than failing. The
//! local-interface multi-component test needs only `wac`.

use std::path::{Path, PathBuf};
use std::process::Command;

use wavelet::scaffold::{self, ProjectKind};
use wavelet::tools;

/// A fresh temp directory unique to this test, cleaned on entry and exit.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-compose-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Is `bin` runnable (`<bin> --version` succeeds)?
fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Can this environment reach the registry `wkg` fetches from? Probes with a
/// throwaway world so an offline CI skips rather than fails. `world_export` is a
/// concrete host interface to pull (`wasi:cli/run@0.2.0`, …).
fn registry_reachable(dir: &Path, world_export: &str) -> bool {
    let probe = dir.join("probe/wit");
    if std::fs::create_dir_all(&probe).is_err() {
        return false;
    }
    let wit = format!("package probe:p@0.1.0;\nworld p {{ export {world_export}; }}\n");
    if std::fs::write(probe.join("p.wit"), wit).is_err() {
        return false;
    }
    tools::wkg_wit_fetch(&probe).is_ok()
}

/// Scaffold `kind` into `proj`, fetch its host WIT (as `wavelet new` does), then
/// build — yielding the composed `<out>/app.wasm`. Returns its path.
fn scaffold_and_build(proj: &Path, kind: ProjectKind, entry: &str) -> PathBuf {
    scaffold::create(proj.to_str().unwrap(), kind).expect("scaffold project");

    let src_paths = vec![proj.join("src/greeting.wvl"), proj.join(entry)];
    wavelet::build::populate_project_wit(proj, &src_paths).expect("populate wit/deps via wkg");

    let out = proj.join("out");
    let sources: Vec<String> = src_paths.iter().map(|p| p.to_str().unwrap().to_string()).collect();
    let outputs =
        wavelet::build::build_files(&sources, out.to_str().unwrap()).expect("build components");

    // The headline output is one composed artifact, alongside the per-component
    // wasm the build still emits.
    let app = out.join("app.wasm");
    assert!(app.is_file(), "build did not produce a composed app.wasm: {outputs:?}");
    app
}

/// The `cli` template builds to one composed component that `wasmtime run`
/// executes, greeting the world and the name given on the command line.
#[test]
fn cli_template_builds_and_runs() {
    if !have("wkg") || !have("wac") || !have("wasmtime") {
        eprintln!("skipping: wkg/wac/wasmtime not all on PATH");
        return;
    }
    let dir = scratch("cli");
    if !registry_reachable(&dir, "wasi:cli/run@0.2.0") {
        eprintln!("skipping: registry unreachable");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let app = scaffold_and_build(&dir.join("greeter"), ProjectKind::Cli, "src/main.wvl");

    let run = |args: &[&str]| -> String {
        let out = Command::new("wasmtime")
            .arg("run")
            .arg(&app)
            .args(args)
            .output()
            .expect("run wasmtime");
        assert!(out.status.success(), "wasmtime run failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    assert_eq!(run(&[]).trim(), "Hello, world!");
    assert_eq!(run(&["Ada"]).trim(), "Hello, Ada!");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The `http` template builds to one composed component that `wasmtime serve`
/// answers requests with, rendering the greeting and echoing the request path.
#[test]
fn http_template_builds_and_serves() {
    if !have("wkg") || !have("wac") || !have("wasmtime") {
        eprintln!("skipping: wkg/wac/wasmtime not all on PATH");
        return;
    }
    let dir = scratch("http");
    if !registry_reachable(&dir, "wasi:http/incoming-handler@0.2.0") {
        eprintln!("skipping: registry unreachable");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let app = scaffold_and_build(&dir.join("web"), ProjectKind::Http, "src/app.wvl");

    // The composed component is a real wasi:http proxy: it exports the handler
    // and imports wasi:http/types (the greeting dep is composed *in*, so it is no
    // longer an import of the final artifact).
    // Decode the component's WIT and assert on the world's import/export lines
    // (a raw-byte search would false-match the *embedded* greeting package text).
    let wit = component_wit(&app);
    assert!(wit.contains("export wasi:http/incoming-handler"), "app does not export the handler:\n{wit}");
    assert!(wit.contains("import wasi:http/types"), "app does not import wasi:http/types:\n{wit}");
    assert!(
        !wit.contains("import web:greeting"),
        "greeting dep was not composed in (still imported):\n{wit}"
    );

    // Serve it on an unprivileged port and make one request.
    let addr = "127.0.0.1:8753";
    let mut child = Command::new("wasmtime")
        .args(["serve", "--addr", addr])
        .arg(&app)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn wasmtime serve");

    // Poll until the server answers (or give up).
    let body = poll_get(&format!("http://{addr}/hello/path"));
    let _ = child.kill();
    let _ = child.wait();

    let body = body.expect("server never answered");
    assert!(body.contains("Hello, world!"), "missing greeting in page: {body}");
    assert!(body.contains("You requested: /hello/path"), "missing echoed path: {body}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A multi-component project of two local-interface components (`demo:main`
/// imports `demo:shout/api`, exported by `demo:shout`) composes to a single
/// component: `wac` embeds `demo:shout` and leaves only `demo:main`'s own export
/// on the final artifact. Needs only `wac` (no host WIT to fetch).
#[test]
fn multi_component_composes_to_one() {
    if !have("wac") {
        eprintln!("skipping: wac not on PATH");
        return;
    }
    let dir = scratch("multi");
    let src = dir.join("demo/src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("shout.wvl"),
        "Package \"demo:shout@0.1.0\"\n\n\
         Export shout\n\
         Def shout Fn {phrase: string}\n  str-cat(upper(phrase) \"!\")\n",
    )
    .unwrap();
    std::fs::write(
        src.join("main.wvl"),
        "Package \"demo:main@0.1.0\"\n\n\
         Import {pkg: \"demo:shout/api\" as: sh}\n\n\
         Export {name: run params: {} result: string}\n\
         Def run Fn {}\n  sh/shout({phrase: \"hello\"})\n",
    )
    .unwrap();

    let out = dir.join("demo/out");
    let sources = vec![
        src.join("shout.wvl").to_str().unwrap().to_string(),
        src.join("main.wvl").to_str().unwrap().to_string(),
    ];
    let outputs =
        wavelet::build::build_files(&sources, out.to_str().unwrap()).expect("build demo components");

    let app = out.join("app.wasm");
    assert!(app.is_file(), "multi-component build produced no app.wasm: {outputs:?}");

    // The composed component must embed demo:shout (no longer an import) and keep
    // demo:main's own interface as its export.
    let wit = String::from_utf8_lossy(&std::fs::read(&app).unwrap()).into_owned();
    assert!(wit.contains("demo:main"), "composed component lost demo:main export");
    assert!(
        !wit.contains("import demo:shout"),
        "demo:shout was not composed in (still imported): {wit}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Decode a composed component back into its WIT text (the world plus its
/// imports/exports), so tests can assert on real wiring rather than raw bytes.
fn component_wit(path: &Path) -> String {
    use wit_component::WitPrinter;
    let bytes = std::fs::read(path).expect("read component");
    let decoded = wit_component::decode(&bytes).expect("decode component");
    let resolve = decoded.resolve();
    let pkg = decoded.package();
    let mut printer = WitPrinter::default();
    printer.print(resolve, pkg, &[]).expect("print wit");
    printer.output.to_string()
}

/// Issue a GET to `url`, retrying briefly while the freshly-spawned server comes
/// up. Returns the response body on the first success, `None` if it never
/// answered. Uses a tiny hand-rolled HTTP/1.0 client to avoid a dependency.
fn poll_get(url: &str) -> Option<String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a.to_string(), format!("/{p}")),
        None => (rest.to_string(), "/".to_string()),
    };

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(mut stream) = TcpStream::connect(&authority) {
            let req = format!(
                "GET {path} HTTP/1.0\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
            );
            if stream.write_all(req.as_bytes()).is_ok() {
                let mut buf = String::new();
                if stream.read_to_string(&mut buf).is_ok() && !buf.is_empty() {
                    // Strip headers; return the body.
                    if let Some(idx) = buf.find("\r\n\r\n") {
                        return Some(buf[idx + 4..].to_string());
                    }
                    return Some(buf);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}
