//! End-to-end build test for the `http` project template, via the *generic*
//! WIT bridge (Step 8 of the WASI-decoupling plan).
//!
//! The http front end is no longer compiled by the hand-coded `http/*` intrinsic
//! magic: it imports `wasi:http/types` and `wasi:io/streams` as ordinary WIT
//! packages (fetched into `wit/deps` by `wkg`), calls their functions through the
//! generic canonical-ABI bridge, and exports `wasi:http/incoming-handler` through
//! the generic export path. This test scaffolds the template, populates its WIT
//! with `wkg`, builds it, and checks the component is a real wasi:http proxy:
//! it embeds and validates through the component encoder, imports
//! `wasi:http/types`, and exports `wasi:http/incoming-handler`.
//!
//! Because the generic path needs the real WIT under `wit/deps`, the test is
//! gated on `wkg` being installed *and* a registry fetch succeeding — it skips
//! cleanly in toolless/offline CI rather than failing.
//!
//! It does not run the component (that needs a host like `wasmtime serve`); the
//! `README`/`scripts/serve.sh` path is the manual check for actually serving.

use wavelet::scaffold::{self, ProjectKind};
use wavelet::tools::{self, Tool};

/// A fresh temp directory unique to this test, cleaned on entry and exit.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-http-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn http_template_builds_a_wasi_http_proxy_component() {
    if tools::version(Tool::Wkg).is_err() {
        eprintln!("skipping: wkg not on PATH");
        return;
    }

    let dir = scratch("build");
    let proj = dir.join("widgets");
    let proj_str = proj.to_str().unwrap();

    scaffold::create(proj_str, ProjectKind::Http).expect("scaffold http project");

    // Probe whether this environment can reach the registry. If not (offline CI),
    // skip without failing — the generic path needs the real wasi WIT.
    {
        let probe = dir.join("probe/wit");
        std::fs::create_dir_all(&probe).unwrap();
        std::fs::write(
            probe.join("p.wit"),
            "package probe:p@0.1.0;\nworld p { export wasi:http/incoming-handler@0.2.0; }\n",
        )
        .unwrap();
        if tools::wkg_wit_fetch(&probe).is_err() {
            eprintln!("skipping: wkg present but registry unreachable");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
    }

    // Populate the project's wit/deps from wkg, exactly as `wavelet new` does.
    let src_paths = vec![
        proj.join("src/greeting.wvl"),
        proj.join("src/app.wvl"),
    ];
    wavelet::build::populate_project_wit(&proj, &src_paths).expect("populate wit/deps via wkg");

    let out = dir.join("out");
    let out_str = out.to_str().unwrap();
    let sources: Vec<String> = src_paths.iter().map(|p| p.to_str().unwrap().to_string()).collect();

    // Building runs each file through the component encoder with validation on;
    // a wrong canonical-ABI lowering for any wasi:http / wasi:io call fails here.
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
