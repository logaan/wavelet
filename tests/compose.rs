use std::path::{Path, PathBuf};
use std::process::Command;

use wavelet::scaffold::{self, ProjectKind};
use wavelet::tools;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-compose-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

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

fn scaffold_and_build(proj: &Path, kind: ProjectKind, entry: &str) -> PathBuf {
    scaffold::create(proj.to_str().unwrap(), kind).expect("scaffold project");

    let src_paths = vec![proj.join("src/greeting.wlt"), proj.join(entry)];
    wavelet::build::populate_project_wit(proj, &src_paths).expect("populate wit/deps via wkg");

    let out = proj.join("out");
    let sources: Vec<String> = src_paths
        .iter()
        .map(|p| p.to_str().unwrap().to_string())
        .collect();
    let outputs =
        wavelet::build::build_files(&sources, out.to_str().unwrap()).expect("build components");

    let app = out.join("app.wasm");
    assert!(
        app.is_file(),
        "build did not produce a composed app.wasm: {outputs:?}"
    );
    app
}

#[test]
fn cli_template_builds_and_runs() {
    if !have("wkg") || !have("wac") || !have("wasmtime") {
        assert!(
            std::env::var_os("WAVELET_REQUIRE_HOST_TOOLS").is_none(),
            "wkg/wac/wasmtime must be installed for required host tests"
        );
        eprintln!("skipping: wkg/wac/wasmtime not all on PATH");
        return;
    }
    let dir = scratch("cli");
    if !registry_reachable(&dir, "wasi:cli/run@0.2.0") {
        assert!(
            std::env::var_os("WAVELET_REQUIRE_HOST_TOOLS").is_none(),
            "registry must be reachable for required host tests"
        );
        eprintln!("skipping: registry unreachable");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let app = scaffold_and_build(&dir.join("greeter"), ProjectKind::Cli, "src/main.wlt");

    let run = |args: &[&str]| -> String {
        let out = Command::new("wasmtime")
            .arg("run")
            .arg(&app)
            .args(args)
            .output()
            .expect("run wasmtime");
        assert!(
            out.status.success(),
            "wasmtime run failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    assert_eq!(run(&[]).trim(), "Hello, world!");
    assert_eq!(run(&["Ada"]).trim(), "Hello, Ada!");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn http_template_builds_and_serves() {
    if !have("wkg") || !have("wac") || !have("wasmtime") {
        assert!(
            std::env::var_os("WAVELET_REQUIRE_HOST_TOOLS").is_none(),
            "wkg/wac/wasmtime must be installed for required host tests"
        );
        eprintln!("skipping: wkg/wac/wasmtime not all on PATH");
        return;
    }
    let dir = scratch("http");
    if !registry_reachable(&dir, "wasi:http/incoming-handler@0.2.0") {
        assert!(
            std::env::var_os("WAVELET_REQUIRE_HOST_TOOLS").is_none(),
            "registry must be reachable for required host tests"
        );
        eprintln!("skipping: registry unreachable");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let app = scaffold_and_build(&dir.join("web"), ProjectKind::Http, "src/app.wlt");

    let wit = component_wit(&app);
    assert!(
        wit.contains("export wasi:http/incoming-handler"),
        "app does not export the handler:\n{wit}"
    );
    assert!(
        wit.contains("import wasi:http/types"),
        "app does not import wasi:http/types:\n{wit}"
    );
    assert!(
        !wit.contains("import web:greeting"),
        "greeting dep was not composed in (still imported):\n{wit}"
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let mut child = Command::new("wasmtime")
        .args(["serve", "--addr", &addr])
        .arg(&app)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn wasmtime serve");

    let body = poll_get(&format!("http://{addr}/hello/path"));
    let escaped = poll_get(&format!("http://{addr}/hello?a=1&b=2"));
    let _ = child.kill();
    let _ = child.wait();

    let body = body.expect("server never answered");
    let escaped = escaped.expect("server never answered second request");
    assert!(escaped.contains("/hello?a=1&amp;b=2"), "{escaped}");
    assert!(
        body.to_lowercase()
            .contains("content-type: text/html; charset=utf-8"),
        "{body}"
    );
    assert!(
        body.starts_with("HTTP/1.0 200") || body.starts_with("HTTP/1.1 200"),
        "{body}"
    );
    assert!(
        escaped.starts_with("HTTP/1.0 200") || escaped.starts_with("HTTP/1.1 200"),
        "{escaped}"
    );
    assert!(
        body.contains("Hello, world!"),
        "missing greeting in page: {body}"
    );
    assert!(
        body.contains("You requested: /hello/path"),
        "missing echoed path: {body}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

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
        src.join("shout.wlt"),
        "Package \"demo:shout@0.1.0\"\n\n\
         Export shout\n\
         Def shout Fn {phrase: string}\n  str-cat(phrase \"!\")\n",
    )
    .unwrap();
    std::fs::write(
        src.join("main.wlt"),
        "Package \"demo:main@0.1.0\"\n\n\
         Import {pkg: \"demo:shout/api\" as: sh}\n\n\
         Export {name: run params: {} result: string}\n\
         Def run Fn {}\n  sh/shout({phrase: \"hello\"})\n",
    )
    .unwrap();

    let out = dir.join("demo/out");
    let sources = vec![
        src.join("shout.wlt").to_str().unwrap().to_string(),
        src.join("main.wlt").to_str().unwrap().to_string(),
    ];
    let outputs = wavelet::build::build_files(&sources, out.to_str().unwrap())
        .expect("build demo components");

    let app = out.join("app.wasm");
    assert!(
        app.is_file(),
        "multi-component build produced no app.wasm: {outputs:?}"
    );

    let wit = String::from_utf8_lossy(&std::fs::read(&app).unwrap()).into_owned();
    assert!(
        wit.contains("demo:main"),
        "composed component lost demo:main export"
    );
    assert!(
        !wit.contains("import demo:shout"),
        "demo:shout was not composed in (still imported): {wit}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

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
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .ok()?;
            let req =
                format!("GET {path} HTTP/1.0\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
            if stream.write_all(req.as_bytes()).is_ok() {
                let mut buf = String::new();
                if stream.read_to_string(&mut buf).is_ok() && !buf.is_empty() {
                    return Some(buf);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}
