use wavelet::form::{Arena, Node, NodeId};
use wavelet::host::{HostComponent, Val};
use wavelet::printer::print;
use wavelet::reader::read_file;

const EXPAND: &str = "`expand` is only available inside a macro library when building (6.1.8)";

const SKIP: &[(&str, &str)] = &[("macro-expand", EXPAND)];

const PACKAGE: &str = "docs:snippet@0.1.0";
const IFACE: &str = "docs:snippet/api@0.1.0";
const MAIN: &str = "differential-main";

const DECL_HEADS: &[&str] = &[
    "package-MACRO",
    "target-MACRO",
    "import-MACRO",
    "export-MACRO",
    "def-MACRO",
    "defmacro-MACRO",
    "deftype-MACRO",
    "defresource-MACRO",
    "derive-MACRO",
];

fn is_decl(arena: &Arena, root: NodeId) -> bool {
    let Node::Tup(items) = arena.node(root) else {
        return false;
    };
    let Some(&head) = items.first() else {
        return false;
    };
    matches!(arena.node(head), Node::Sym(s) if DECL_HEADS.contains(&s.as_str()))
}

fn wrap_snippet(code: &str) -> Result<String, String> {
    let (arena, roots) = read_file(code).map_err(|e| format!("read: {e}"))?;
    let mut decls = Vec::new();
    let mut exprs = Vec::new();
    for &root in &roots {
        if is_decl(&arena, root) {
            decls.push(print(&arena, root));
            if let Node::Tup(items) = arena.node(root)
                && matches!(arena.node(items[0]), Node::Sym(head) if head == "defresource-MACRO")
                && let Some(name) = items.get(1)
            {
                decls.push(format!("Export {}", print(&arena, *name)));
            }
        } else {
            exprs.push(print(&arena, root));
        }
    }
    let ends_in_expr = roots.last().is_some_and(|&r| !is_decl(&arena, r));

    let mut out = format!("Package \"{PACKAGE}\"\n\n");
    for d in &decls {
        out.push_str(d);
        out.push('\n');
    }
    out.push_str(&format!("\nExport {{name: {MAIN} result: string}}\n"));
    out.push_str(&format!("Def {MAIN} Fn {{}}\n"));
    let body = if ends_in_expr {
        format!("to-string(Do [{}])", exprs.join("\n    "))
    } else if exprs.is_empty() {
        "\"\"".to_string()
    } else {
        format!("Do [{}\n    \"\"]", exprs.join("\n    "))
    };
    out.push_str(&format!("  {body}\n"));
    Ok(out)
}

fn compiled_eval(_id: &str, program: &str) -> Result<(String, String), String> {
    let bytes = wavelet::staging::Project::compile(program).map_err(|e| e.to_string())?;
    let mut component =
        HostComponent::from_bytes(&bytes).map_err(|e| format!("instantiate: {e}"))?;
    wavelet::output_capture_start();
    let result = component.call_instance(IFACE, MAIN, &[]);
    let output = wavelet::output_capture_take();
    let vals = result.map_err(|e| format!("call: {e}"))?;
    match vals.as_slice() {
        [Val::String(s)] => Ok((s.to_string(), output)),
        other => Err(format!("call: unexpected result shape {other:?}")),
    }
}

enum Outcome {
    Agree,

    Disagree(String),
}

fn run_example(id: &str, code: &str, expect_error: bool) -> Outcome {
    let interp = wavelet::eval_snippet(code);
    assert_eq!(
        interp.ok, !expect_error,
        "`{id}`: interpreter outcome drifted from examples.json — \
         tests/examples.rs should be failing too"
    );

    let compiled = wrap_snippet(code).and_then(|program| compiled_eval(id, &program));

    match (expect_error, compiled) {
        (true, Err(_)) => Outcome::Agree,
        (true, Ok(v)) => Outcome::Disagree(format!(
            "interpreter errors (`{}`) but the compiled artifact succeeds with {v:?}",
            interp.error
        )),
        (false, Err(e)) => Outcome::Disagree(format!(
            "interpreter succeeds with {:?} but the compiled artifact fails at {e}",
            interp.value
        )),
        (false, Ok((v, output))) => {
            if output != interp.output {
                return Outcome::Disagree(format!(
                    "stdout mismatch: {:?} != {output:?}",
                    interp.output
                ));
            }

            if v == interp.value || (interp.value.is_empty() && v == "{}") {
                Outcome::Agree
            } else {
                Outcome::Disagree(format!(
                    "value mismatch\n    interpreter: {:?}\n    compiled:    {v:?}",
                    interp.value
                ))
            }
        }
    }
}

#[test]
fn every_documented_example_agrees_with_the_compiled_artifact() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples.json");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    let examples: serde_json::Value =
        serde_json::from_str(&text).expect("docs/examples.json is not valid JSON");
    let map = examples
        .as_object()
        .expect("examples.json must be an object");
    assert!(!map.is_empty(), "no examples found");

    let mut ids: Vec<&String> = map.keys().collect();
    ids.sort();

    let mut divergences = Vec::new();
    let mut ratchet = Vec::new();
    for id in ids {
        let entry = &map[id.as_str()];
        let code = entry["code"].as_str().unwrap_or_else(|| {
            panic!("example `{id}` has no string `code`");
        });
        let expect_error = entry.get("error").is_some();
        let skip = SKIP.iter().find(|(s, _)| s == id).map(|(_, why)| *why);

        match (run_example(id, code, expect_error), skip) {
            (Outcome::Agree, None) => {}
            (Outcome::Agree, Some(_)) => ratchet.push(id.clone()),
            (Outcome::Disagree(_), Some(_)) => {}
            (Outcome::Disagree(how), None) => {
                divergences.push(format!("`{id}`: {how}"));
            }
        }
    }

    let mut report = String::new();
    if !divergences.is_empty() {
        report.push_str(&format!(
            "\n{} example(s) diverged between the interpreter and the compiled artifact:\n\n{}\n",
            divergences.len(),
            divergences.join("\n\n"),
        ));
    }
    if !ratchet.is_empty() {
        report.push_str(&format!(
            "\n{} SKIP-listed example(s) now agree — remove them from SKIP in {}:\n  {}\n",
            ratchet.len(),
            file!(),
            ratchet.join("\n  "),
        ));
    }
    assert!(report.is_empty(), "{report}");
}

#[test]
fn independent_execution_corpus_agrees() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/execution");
    let mut paths: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    assert!(!paths.is_empty());
    for path in paths {
        let code = std::fs::read_to_string(&path).unwrap();
        let id = path.file_stem().unwrap().to_str().unwrap();
        if let Outcome::Disagree(message) = run_example(id, &code, id.ends_with("-error")) {
            panic!("{id}: {message}");
        }
    }
}
