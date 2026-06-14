//! End-to-end build test for the `http` project template.
//!
//! Scaffolds the `--type=http` template, compiles its two components, and
//! checks that the front end is a real wasi:http proxy component: it embeds and
//! validates through the component encoder (so the canonical-ABI signatures of
//! the `http/*` intrinsics are correct), imports `wasi:http/types`, and exports
//! `wasi:http/incoming-handler`. This guards against drift in the vendored WASI
//! WIT or the resource/intrinsic emit paths.
//!
//! It does not run the component (that needs a host like `wasmtime serve`); the
//! `README`/`scripts/serve.sh` path is the manual check for actually serving.

use wavelet::scaffold::{self, ProjectKind};

/// A fresh temp directory unique to this test, cleaned on entry and exit.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-http-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn http_template_builds_a_wasi_http_proxy_component() {
    let dir = scratch("build");
    let proj = dir.join("widgets");
    let proj_str = proj.to_str().unwrap();

    scaffold::create(proj_str, ProjectKind::Http).expect("scaffold http project");

    let out = dir.join("out");
    let out_str = out.to_str().unwrap();
    let sources = vec![
        proj.join("src/greeting.wvl").to_str().unwrap().to_string(),
        proj.join("src/app.wvl").to_str().unwrap().to_string(),
    ];

    // Building runs each file through the component encoder with validation on;
    // a wrong canonical-ABI signature for any `http/*` intrinsic fails here.
    let outputs = wavelet::build::build_files(&sources, out_str).expect("build http components");
    assert_eq!(outputs.len(), 2, "expected one component per source file");

    let app = out.join("widgets-app.wasm");
    let bytes = std::fs::read(&app).expect("read app component");

    // The component's embedded type section names the interfaces it wires; the
    // front end must export the incoming-handler and import wasi:http/types.
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("wasi:http/incoming-handler"),
        "app component does not export wasi:http/incoming-handler"
    );
    assert!(
        text.contains("wasi:http/types"),
        "app component does not import wasi:http/types"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
