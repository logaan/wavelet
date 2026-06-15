//! Step 2 of the WASI-decoupling plan: `wavelet build`/`wavelet new` synthesize
//! the project's own WIT into `wit/` and run `wkg wit fetch` to populate
//! `wit/deps` and write `wkg.lock` — behind the scenes, with codegen unchanged.
//!
//! The synthesis assertions are hermetic (no tools, no network). The actual
//! `wkg` fetch is exercised by a single gated test that skips cleanly when `wkg`
//! is absent, so the suite stays green in toolless/offline CI.

use std::path::PathBuf;

use wavelet::scaffold::{self, ProjectKind};
use wavelet::tools::{self, Tool};
use wavelet::{expand, read_file, wit};

/// A fresh temp directory unique to this test, cleaned on entry and exit.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-wkg-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn fetch_world(src: &str) -> String {
    let (arena, roots) = read_file(src).expect("read");
    let (arena, roots) = expand::expand_file(arena, &roots).expect("expand");
    wit::synthesize_fetch_world(&arena, &roots).expect("synthesize_fetch_world")
}

fn collect(src: &str) -> wit::FileInfo {
    let (arena, roots) = read_file(src).expect("read");
    let (arena, roots) = expand::expand_file(arena, &roots).expect("expand");
    wit::collect(&arena, &roots).expect("collect")
}

// --- hermetic: the fetch world is host-only and registry-resolvable ---------

/// The http front end's fetch world references only host (`wasi:*`) packages —
/// no sibling `widgets:greeting` import, which `wkg` could not resolve from a
/// registry — and keeps the external incoming-handler export.
#[test]
fn http_fetch_world_is_host_only() {
    let proj = scratch("http-synth").join("widgets");
    scaffold::create(proj.to_str().unwrap(), ProjectKind::Http).unwrap();
    let app = std::fs::read_to_string(proj.join("src/app.wvl")).unwrap();

    let world = fetch_world(&app);
    assert!(world.contains("import wasi:http/types@0.2.0;"), "{world}");
    assert!(
        world.contains("export wasi:http/incoming-handler@0.2.0;"),
        "{world}"
    );
    // The sibling build-set dependency must be dropped from the fetch world.
    assert!(!world.contains("greeting"), "sibling import leaked: {world}");

    let _ = std::fs::remove_dir_all(proj.parent().unwrap());
}

/// The cli entry exports `wasi:cli/run` directly (Step 9 routes it through the
/// generic export path rather than a `Target "wasi:cli/command"` translation),
/// so the fetch world references that concrete interface — which makes `wkg`
/// pull the whole `wasi:cli` package — and drops the sibling greeting import.
#[test]
fn cli_fetch_world_references_wasi_cli_run() {
    let proj = scratch("cli-synth").join("widgets");
    scaffold::create(proj.to_str().unwrap(), ProjectKind::Cli).unwrap();
    let main = std::fs::read_to_string(proj.join("src/main.wvl")).unwrap();

    let world = fetch_world(&main);
    assert!(world.contains("export wasi:cli/run@0.2.0;"), "{world}");
    assert!(!world.contains("include"), "include leaked into fetch world: {world}");
    assert!(!world.contains("greeting"), "sibling import leaked: {world}");

    let _ = std::fs::remove_dir_all(proj.parent().unwrap());
}

/// `has_host_deps` distinguishes a component that needs fetching (the cli/http
/// entry, with a target / wasi import) from a pure domain model (greeting).
#[test]
fn has_host_deps_only_flags_components_with_wasi() {
    let proj = scratch("hostdeps").join("widgets");
    scaffold::create(proj.to_str().unwrap(), ProjectKind::Http).unwrap();
    let app = std::fs::read_to_string(proj.join("src/app.wvl")).unwrap();
    let greeting = std::fs::read_to_string(proj.join("src/greeting.wvl")).unwrap();

    assert!(wit::has_host_deps(&collect(&app)), "http app should need fetching");
    assert!(
        !wit::has_host_deps(&collect(&greeting)),
        "pure greeting should not need fetching"
    );

    let _ = std::fs::remove_dir_all(proj.parent().unwrap());
}

// --- live fetch (gated): requires `wkg` + registry access -------------------

/// Building the cli template actually populates `wit/deps` and writes
/// `wkg.lock`. Gated on `wkg` being installed *and* a fetch succeeding, so it
/// skips cleanly in toolless/offline CI rather than failing the suite.
#[test]
fn build_populates_wit_deps_and_lock() {
    if tools::version(Tool::Wkg).is_err() {
        eprintln!("skipping: wkg not on PATH");
        return;
    }

    let dir = scratch("livebuild");
    let proj = dir.join("widgets");
    scaffold::create(proj.to_str().unwrap(), ProjectKind::Cli).expect("scaffold");

    // Probe whether this environment can actually reach the registry, using a
    // throwaway project. If it can't (offline CI), skip without failing.
    {
        let probe = dir.join("probe/wit");
        std::fs::create_dir_all(&probe).unwrap();
        std::fs::write(
            probe.join("p.wit"),
            "package probe:p@0.1.0;\nworld p { export wasi:cli/run@0.2.0; }\n",
        )
        .unwrap();
        if tools::wkg_wit_fetch(&probe).is_err() {
            eprintln!("skipping: wkg present but registry unreachable");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
    }

    // Fetch the cli template's host WIT into `wit/deps` first, exactly as
    // `wavelet new` does — the template now imports `wasi:cli/stdout`,
    // `wasi:cli/environment`, and `wasi:io/streams` directly, so their parsed
    // WIT must be present before emit can lower the calls through the generic
    // bridge.
    let src_paths = vec![proj.join("src/greeting.wvl"), proj.join("src/main.wvl")];
    wavelet::build::populate_project_wit(&proj, &src_paths).expect("populate wit/deps via wkg");

    let out = proj.join("out");
    let sources = vec![
        proj.join("src/greeting.wvl").to_str().unwrap().to_string(),
        proj.join("src/main.wvl").to_str().unwrap().to_string(),
    ];
    wavelet::build::build_files(&sources, out.to_str().unwrap()).expect("build");

    let deps = proj.join("wit/deps");
    assert!(deps.is_dir(), "wit/deps was not created");
    let entries: Vec<_> = std::fs::read_dir(&deps)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        entries.iter().any(|n| n.starts_with("wasi-cli")),
        "wit/deps missing wasi-cli: {entries:?}"
    );
    assert!(proj.join("wkg.lock").is_file(), "wkg.lock was not written");

    let _ = std::fs::remove_dir_all(&dir);
}
