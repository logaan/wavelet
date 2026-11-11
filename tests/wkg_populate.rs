use std::path::PathBuf;

use wavelet::scaffold::{self, ProjectKind};
use wavelet::tools::{self, Tool};
use wavelet::{expand, read_file, wit};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-wkg-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn fetch_world(src: &str) -> String {
    let (arena, roots) = read_file(src).expect("read");
    let (arena, roots) = expand::expand_file(arena, &roots, None).expect("expand");
    wit::synthesize_fetch_world_excluding(&arena, &roots, &["widgets:greeting".to_owned()])
        .expect("synthesize_fetch_world")
}

fn collect(src: &str) -> wit::FileInfo {
    let (arena, roots) = read_file(src).expect("read");
    let (arena, roots) = expand::expand_file(arena, &roots, None).expect("expand");
    wit::collect(&arena, &roots).expect("collect")
}

#[test]
fn http_fetch_world_is_host_only() {
    let proj = scratch("http-synth").join("widgets");
    scaffold::create(proj.to_str().unwrap(), ProjectKind::Http).unwrap();
    let app = std::fs::read_to_string(proj.join("src/app.wlt")).unwrap();

    let world = fetch_world(&app);
    assert!(world.contains("import wasi:http/types@0.2.0;"), "{world}");
    assert!(
        world.contains("export wasi:http/incoming-handler@0.2.0;"),
        "{world}"
    );

    assert!(
        !world.contains("greeting"),
        "sibling import leaked: {world}"
    );

    let _ = std::fs::remove_dir_all(proj.parent().unwrap());
}

#[test]
fn cli_fetch_world_references_wasi_cli_run() {
    let proj = scratch("cli-synth").join("widgets");
    scaffold::create(proj.to_str().unwrap(), ProjectKind::Cli).unwrap();
    let main = std::fs::read_to_string(proj.join("src/main.wlt")).unwrap();

    let world = fetch_world(&main);
    assert!(world.contains("export wasi:cli/run@0.2.0;"), "{world}");
    assert!(
        !world.contains("include"),
        "include leaked into fetch world: {world}"
    );
    assert!(
        !world.contains("greeting"),
        "sibling import leaked: {world}"
    );

    let _ = std::fs::remove_dir_all(proj.parent().unwrap());
}

#[test]
fn external_dependency_detection_is_namespace_independent() {
    let proj = scratch("hostdeps").join("widgets");
    scaffold::create(proj.to_str().unwrap(), ProjectKind::Http).unwrap();
    let app = std::fs::read_to_string(proj.join("src/app.wlt")).unwrap();
    let greeting = std::fs::read_to_string(proj.join("src/greeting.wlt")).unwrap();

    assert!(
        wit::has_external_deps(&collect(&app), &[]),
        "http app should need fetching"
    );
    assert!(
        !wit::has_external_deps(&collect(&greeting), &[]),
        "pure greeting should not need fetching"
    );

    let _ = std::fs::remove_dir_all(proj.parent().unwrap());
}

#[test]
fn build_populates_wit_deps_and_lock() {
    if tools::version(Tool::Wkg).is_err() {
        eprintln!("skipping: wkg not on PATH");
        return;
    }

    let dir = scratch("livebuild");
    let proj = dir.join("widgets");
    scaffold::create(proj.to_str().unwrap(), ProjectKind::Cli).expect("scaffold");

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

    let src_paths = vec![proj.join("src/greeting.wlt"), proj.join("src/main.wlt")];
    wavelet::build::populate_project_wit(&proj, &src_paths).expect("populate wit/deps via wkg");

    let out = proj.join("out");
    let sources = vec![
        proj.join("src/greeting.wlt").to_str().unwrap().to_string(),
        proj.join("src/main.wlt").to_str().unwrap().to_string(),
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
