//! 0.13 — the docs-independent differential fixture corpus.
//!
//! `tests/differential.rs` runs every *documented* example through both
//! engines but compares only the final printed value. This harness closes its
//! two blind spots: it exercises surfaces the docs corpus never touches
//! (resource lifecycle across calls, cell mutation ordering, arithmetic
//! edges, out-of-bounds access, deep tail recursion, functor `SET_OPS`, …)
//! and it compares the **ordered observable stream**, not just a final value.
//!
//! The language has no `print` builtin and `run` discards results, so the
//! "stream" is defined here exactly as the step-6 plan specifies:
//!
//! * **Generic fixtures** (`tests/fixtures/differential/*.wlt`): the ordered
//!   sequence of top-level expression values as printed (what the REPL
//!   surfaces). Each fixture is wrapped into a synthetic component exporting
//!   one `exprN: func() -> string` per top-level expression (guest-side
//!   `to-string`, the port of the interpreter's `print_value`); the harness
//!   calls `expr0..exprN` in order on ONE live instance and compares the
//!   resulting stream with a runner-style interpreter drive of the same file
//!   (per-root `print_value`). An error must land on both sides at the same
//!   position: the successfully-produced prefixes must be equal.
//!
//! * **Multi-call fixtures** (`tests/fixtures/differential/multicall/*.wlt`):
//!   complete programs with their own `Package`/`Export`s, driven by an
//!   explicit script of repeated host calls against ONE live instance —
//!   resource handles and functor-set handles are threaded between calls on
//!   both engines, so state that must persist across the call boundary
//!   (interpreter `Rc<RefCell>` env vs backend persist region) is compared
//!   per call, in order.
//!
//! Both sides run the same static pipeline the real drivers use (`expand` →
//! `check::resolve_overloads` → eval / `build::build_files`), so a program
//! rejected statically must be rejected statically by both.
//!
//! Every generic fixture declares its expected *shape* in a leading
//! `// EXPECT: ok|error|static` line, so a fixture that silently degrades
//! (e.g. a typo turning an arithmetic probe into a static error that both
//! sides happen to agree on) fails loudly instead of passing vacuously.
//!
//! The skip list is intentionally EMPTY and there is deliberately no skip
//! mechanism: a fixture that exposes a real interpreter/backend divergence is
//! a bug to fix oracle-first (`src/interp.rs` + `src/builtins.rs` define the
//! semantics), not a skip to add. Sister harnesses: `tests/differential.rs`
//! (docs corpus, final value), `tests/macro_differential.rs` (expansion).

use std::rc::Rc;

use wavelet::form::{Arena, Node, NodeId};
use wavelet::host::{HostComponent, Val};
use wavelet::interp::Interp;
use wavelet::printer::print;
use wavelet::reader::read_file;
use wavelet::value::{Env, Value, print_value, unit};
use wavelet::{builtins, check, expand};

const PACKAGE: &str = "diff:fixture@0.1.0";
const IFACE: &str = "diff:fixture/api@0.1.0";

/// Top-level declaration heads (post-reader `-MACRO` spellings): forms that
/// declare rather than evaluate. Everything else is an expression and
/// contributes one position to the observable stream. Superset of the docs
/// harness's list: fixtures also use `DefResource` and `Instantiate`.
const DECL_HEADS: &[&str] = &[
    "package-MACRO",
    "target-MACRO",
    "import-MACRO",
    "instantiate-MACRO",
    "export-MACRO",
    "def-MACRO",
    "defmacro-MACRO",
    "deftype-MACRO",
    "defresource-MACRO",
    "derive-MACRO",
];

fn head_name<'a>(arena: &'a Arena, root: NodeId) -> Option<&'a str> {
    let Node::Tup(items) = arena.node(root) else {
        return None;
    };
    let &head = items.first()?;
    match arena.node(head) {
        Node::Sym(s) => Some(s.as_str()),
        _ => None,
    }
}

fn is_decl(arena: &Arena, root: NodeId) -> bool {
    matches!(head_name(arena, root), Some(h) if DECL_HEADS.contains(&h))
}

/// One engine's observable stream for a fixture.
#[derive(Debug)]
enum Stream {
    /// Every top-level expression evaluated; its printed values, in order.
    Values(Vec<String>),
    /// Evaluation failed at an expression position: the values produced
    /// before the failure (so the failing position is `produced.len()`), and
    /// the engine's error text (reported, never compared — the two engines
    /// word their errors differently).
    ErrorAt { produced: Vec<String>, error: String },
    /// Rejected before anything ran (read / expand / check / build).
    Static(String),
}

/// The expected shape of the (agreeing) outcome, declared in the fixture's
/// `// EXPECT:` header line.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    /// Both engines produce the full value stream.
    Ok,
    /// Both engines fail at the same expression position, after agreeing on
    /// the prefix.
    RuntimeError,
    /// Both engines reject the program before running it.
    Static,
}

fn parse_expect(id: &str, code: &str) -> Expect {
    for line in code.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("// EXPECT:") {
            return match rest.trim() {
                "ok" => Expect::Ok,
                "error" => Expect::RuntimeError,
                "static" => Expect::Static,
                other => panic!("`{id}`: unknown EXPECT shape `{other}`"),
            };
        }
    }
    panic!("`{id}`: fixture has no `// EXPECT: ok|error|static` header line");
}

// ---------------------------------------------------------------------------
// Interpreter side (the semantics oracle)
// ---------------------------------------------------------------------------

/// Run the shared static pipeline the way `wavelet run` does: read, expand to
/// fixpoint (`Derive` is a tree→tree pass the lazy interpreter never
/// dispatches), then type-check + overload-resolve.
fn static_pipeline(code: &str) -> Result<(Rc<Arena>, Vec<NodeId>), String> {
    let (arena, roots) = read_file(code).map_err(|e| format!("read: {e}"))?;
    let (arena, roots) =
        expand::expand_file(arena, &roots, None).map_err(|e| format!("expand: {e}"))?;
    let (arena, roots) =
        check::resolve_overloads(arena, &roots).map_err(|e| format!("check: {e}"))?;
    Ok((Rc::new(arena), roots))
}

/// A single-module interpreter drive, mirroring `runner::eval_module`'s
/// handling of top-level module forms: `Package`/`Export` declare, a functor
/// `Instantiate` binds its `alias/op` names, and everything else evaluates in
/// file order. Each non-declaration root contributes its printed value to the
/// stream.
fn interp_stream(id: &str, code: &str) -> Stream {
    let (arena, roots) = match static_pipeline(code) {
        Ok(pair) => pair,
        Err(e) => return Stream::Static(e),
    };
    let std_env = Env::root();
    builtins::install(&std_env);
    let env = std_env.child();
    let interp = Interp::new();
    let mut produced = Vec::new();
    for &root in &roots {
        match head_name(&arena, root) {
            Some("package-MACRO" | "target-MACRO" | "export-MACRO") => continue,
            Some("instantiate-MACRO") => {
                let Node::Tup(items) = arena.node(root) else {
                    unreachable!()
                };
                let payload = items[1];
                let Some(functor) = builtins::parse_functor_import(&arena, payload) else {
                    return Stream::Static(format!("`{id}`: malformed Instantiate"));
                };
                builtins::bind_functor(&env, &functor);
                continue;
            }
            Some("import-MACRO") => {
                panic!("`{id}`: fixtures must be single-file programs (no `Import`)")
            }
            _ => {}
        }
        let is_expr = !is_decl(&arena, root);
        match interp.eval(&arena, root, &env) {
            Ok(v) => {
                if is_expr {
                    produced.push(print_value(&v));
                }
            }
            Err(e) if is_expr => {
                return Stream::ErrorAt {
                    produced,
                    error: e.to_string(),
                };
            }
            // A declaration failing to *evaluate* is a malformed fixture, not
            // a runtime stream position.
            Err(e) => return Stream::Static(format!("declaration failed: {e}")),
        }
    }
    Stream::Values(produced)
}

// ---------------------------------------------------------------------------
// Compiled side
// ---------------------------------------------------------------------------

/// Wrap a fixture into a self-contained component source: declarations in
/// original order, then one exported `exprN: func() -> string` per top-level
/// expression whose body is `to-string(<expr>)`. The reader's printed forms
/// are canonical and re-readable, so the wrapped program is reconstructed
/// from the parsed tree exactly as `tests/differential.rs` does.
///
/// Returns the wrapped source and the number of expression exports.
fn wrap_fixture(id: &str, code: &str) -> (String, usize) {
    let (arena, roots) = read_file(code).unwrap_or_else(|e| panic!("`{id}`: read: {e}"));
    let mut decls = Vec::new();
    let mut exprs = Vec::new();
    for &root in &roots {
        assert!(
            head_name(&arena, root) != Some("package-MACRO"),
            "`{id}`: generic fixtures must not declare a Package (the wrapper owns it)"
        );
        if is_decl(&arena, root) {
            decls.push(print(&arena, root));
        } else {
            exprs.push(print(&arena, root));
        }
    }
    assert!(
        !exprs.is_empty(),
        "`{id}`: fixture has no top-level expressions — nothing to observe"
    );
    let mut out = format!("Package \"{PACKAGE}\"\n\n");
    for d in &decls {
        out.push_str(d);
        out.push('\n');
    }
    for (k, e) in exprs.iter().enumerate() {
        out.push_str(&format!("\nExport {{name: expr{k} result: string}}\n"));
        out.push_str(&format!(
            "Def expr{k} Fn {{}} The string\n  to-string({e})\n"
        ));
    }
    (out, exprs.len())
}

/// Stage a program into a throwaway project dir and build it through the real
/// emitter, returning the built component. The staging pattern is copied
/// as-is from `tests/differential.rs` (a shared staging harness is session
/// C's job — deliberately not refactored here).
fn build_component(id: &str, program: &str) -> Result<HostComponent, String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "wavelet-diff-fixture-{}-{n}-{id}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).map_err(|e| format!("setup: {e}"))?;
    let path = src.join("fixture.wlt");
    std::fs::write(&path, program).map_err(|e| format!("setup: {e}"))?;
    let out_dir = dir.join("out");
    let result = (|| {
        let outputs = wavelet::build::build_files(
            &[path.to_str().unwrap().to_string()],
            out_dir.to_str().unwrap(),
        )
        .map_err(|e| format!("build: {e}"))?;
        let bytes = std::fs::read(&outputs[0]).map_err(|e| format!("read artifact: {e}"))?;
        HostComponent::from_bytes(&bytes).map_err(|e| format!("instantiate: {e}"))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Execute the wrapped fixture's `expr0..exprN` exports in order on ONE live
/// instance, collecting the printed stream.
fn compiled_stream(id: &str, code: &str) -> Stream {
    let (program, n_exprs) = wrap_fixture(id, code);
    let mut component = match build_component(id, &program) {
        Ok(c) => c,
        Err(e) => return Stream::Static(e),
    };
    let mut produced = Vec::new();
    for k in 0..n_exprs {
        match component.call_instance(IFACE, &format!("expr{k}"), &[]) {
            Ok(vals) => match vals.as_slice() {
                [Val::String(s)] => produced.push(s.to_string()),
                other => {
                    return Stream::Static(format!(
                        "`expr{k}` returned an unexpected shape: {other:?}"
                    ));
                }
            },
            Err(e) => {
                return Stream::ErrorAt {
                    produced,
                    error: e.to_string(),
                };
            }
        }
    }
    Stream::Values(produced)
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// Compare the two engines' streams, and both against the declared shape.
/// `None` means agreement; `Some` describes the divergence.
fn compare(expect: Expect, interp: &Stream, compiled: &Stream) -> Option<String> {
    let shape_of = |s: &Stream| match s {
        Stream::Values(_) => Expect::Ok,
        Stream::ErrorAt { .. } => Expect::RuntimeError,
        Stream::Static(_) => Expect::Static,
    };
    let describe = |s: &Stream| match s {
        Stream::Values(vs) => format!("values {vs:?}"),
        Stream::ErrorAt { produced, error } => format!(
            "error at stream position {} after {produced:?}: {error}",
            produced.len()
        ),
        Stream::Static(e) => format!("static rejection: {e}"),
    };
    match (interp, compiled) {
        (Stream::Values(a), Stream::Values(b)) => {
            if a != b {
                return Some(format!(
                    "stream mismatch\n    interpreter: {a:?}\n    compiled:    {b:?}"
                ));
            }
        }
        (
            Stream::ErrorAt {
                produced: a,
                error: ea,
            },
            Stream::ErrorAt {
                produced: b,
                error: eb,
            },
        ) => {
            if a != b {
                return Some(format!(
                    "both error, but at different points / after different values\n    \
                     interpreter: after {a:?} ({ea})\n    compiled:    after {b:?} ({eb})"
                ));
            }
        }
        (Stream::Static(_), Stream::Static(_)) => {}
        (i, c) => {
            return Some(format!(
                "outcome shape mismatch\n    interpreter: {}\n    compiled:    {}",
                describe(i),
                describe(c)
            ));
        }
    }
    // The engines agree; now hold them both to the fixture's declared shape.
    let got = shape_of(interp);
    if got != expect {
        return Some(format!(
            "engines agree but the fixture declares EXPECT {expect:?}, got {got:?} \
             (interpreter: {})",
            describe(interp)
        ));
    }
    None
}

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/differential")
}

#[test]
fn every_fixture_agrees_on_the_full_stream() {
    let dir = fixtures_dir();
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.expect("read_dir entry").path();
            (path.extension().is_some_and(|x| x == "wlt"))
                .then(|| path.file_stem().unwrap().to_str().unwrap().to_string())
        })
        .collect();
    ids.sort();
    assert!(!ids.is_empty(), "no fixtures found in {}", dir.display());

    let mut divergences = Vec::new();
    for id in &ids {
        let code = std::fs::read_to_string(dir.join(format!("{id}.wlt"))).unwrap();
        let expect = parse_expect(id, &code);
        let interp = interp_stream(id, &code);
        let compiled = compiled_stream(id, &code);
        if let Some(how) = compare(expect, &interp, &compiled) {
            divergences.push(format!("`{id}`: {how}"));
        }
    }
    assert!(
        divergences.is_empty(),
        "\n{} fixture(s) diverged between the interpreter and the compiled artifact:\n\n{}\n",
        divergences.len(),
        divergences.join("\n\n"),
    );
}

// ---------------------------------------------------------------------------
// Multi-call fixtures: repeated exported calls on ONE live instance
// ---------------------------------------------------------------------------

/// A script argument: either an immediate value or a handle produced by an
/// earlier `call_handle` step.
#[derive(Clone, Copy)]
enum A {
    U32(u32),
    S32(i32),
    U32List(&'static [u32]),
    /// The handle stored by the `n`th `call_handle` step.
    H(usize),
}

/// Both engines' view of one fixture instance, with the handles each engine
/// produced so far. The wasm side is ONE `HostComponent` for the whole
/// script; the interpreter side is one env — state must persist across calls
/// on both (backend persist region vs `Rc<RefCell>` env).
struct MultiCall {
    id: &'static str,
    component: HostComponent,
    interp: Interp,
    env: Env,
    ivals: Vec<Value>,
    wvals: Vec<Val>,
}

impl MultiCall {
    /// Load `tests/fixtures/differential/multicall/<id>.wlt` — a complete
    /// program with its own `Package` and `Export`s — into both engines.
    fn load(id: &'static str) -> MultiCall {
        let path = fixtures_dir().join(format!("multicall/{id}.wlt"));
        let code = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let component = build_component(id, &code).unwrap_or_else(|e| panic!("`{id}`: {e}"));
        let (arena, roots) = static_pipeline(&code).unwrap_or_else(|e| panic!("`{id}`: {e}"));
        let std_env = Env::root();
        builtins::install(&std_env);
        let env = std_env.child();
        let interp = Interp::new();
        for &root in &roots {
            match head_name(&arena, root) {
                Some("package-MACRO" | "target-MACRO" | "export-MACRO") => continue,
                Some("instantiate-MACRO") => {
                    let Node::Tup(items) = arena.node(root) else {
                        unreachable!()
                    };
                    let functor = builtins::parse_functor_import(&arena, items[1])
                        .unwrap_or_else(|| panic!("`{id}`: malformed Instantiate"));
                    builtins::bind_functor(&env, &functor);
                }
                _ => {
                    interp
                        .eval(&arena, root, &env)
                        .unwrap_or_else(|e| panic!("`{id}`: declaration failed: {e}"));
                }
            }
        }
        MultiCall {
            id,
            component,
            interp,
            env,
            ivals: Vec::new(),
            wvals: Vec::new(),
        }
    }

    fn interp_args(&self, args: &[A]) -> Value {
        let vals: Vec<Value> = args
            .iter()
            .map(|a| match a {
                A::U32(n) => Value::Int(i64::from(*n)),
                A::S32(n) => Value::Int(i64::from(*n)),
                A::U32List(ns) => {
                    Value::Lst(ns.iter().map(|&n| Value::Int(i64::from(n))).collect())
                }
                A::H(i) => self.ivals[*i].clone(),
            })
            .collect();
        // §4.2 bundling, as `interp::bundle_args`: 0 args ⇒ the empty
        // tuple, 1 ⇒ the value itself, ≥2 ⇒ a tuple.
        match vals.len() {
            1 => vals.into_iter().next().unwrap(),
            _ => Value::Tup(vals),
        }
    }

    fn wasm_args(&self, args: &[A]) -> Vec<Val> {
        args.iter()
            .map(|a| match a {
                A::U32(n) => Val::U32(*n),
                A::S32(n) => Val::S32(*n),
                A::U32List(ns) => Val::List(ns.iter().map(|&n| Val::U32(n)).collect()),
                A::H(i) => self.wvals[*i].clone(),
            })
            .collect()
    }

    fn interp_call(&self, func: &str, args: &[A]) -> Result<Value, String> {
        let f = self
            .env
            .lookup(func)
            .unwrap_or_else(|| panic!("`{}`: `{func}` is not bound", self.id));
        self.interp
            .apply(&f, self.interp_args(args))
            .map_err(|e| e.to_string())
    }

    /// One scripted step both engines must agree on: call `interp_fn` on the
    /// interpreter and `iface#wasm_fn` on the live instance, and compare the
    /// printed results (a call returning nothing is the unit value).
    fn call(&mut self, interp_fn: &str, iface: &str, wasm_fn: &str, args: &[A]) {
        let iv = self
            .interp_call(interp_fn, args)
            .unwrap_or_else(|e| panic!("`{}`: interpreter `{interp_fn}` failed: {e}", self.id));
        let wv = self
            .component
            .call_instance(iface, wasm_fn, &self.wasm_args(args))
            .unwrap_or_else(|e| panic!("`{}`: compiled `{wasm_fn}` failed: {e}", self.id));
        let wv = match wv.as_slice() {
            [] => unit(),
            [v] => val_to_value(self.id, wasm_fn, v),
            other => panic!(
                "`{}`: `{wasm_fn}` returned an unexpected shape: {other:?}",
                self.id
            ),
        };
        assert_eq!(
            print_value(&iv),
            print_value(&wv),
            "`{}`: `{interp_fn}` vs `{wasm_fn}` disagree",
            self.id
        );
    }

    /// A step that produces an opaque handle on both engines (a resource or
    /// functor-set instance). The pair is stored; later steps reference it
    /// with `A::H(index)`. Returns the handle index.
    fn call_handle(&mut self, interp_fn: &str, iface: &str, wasm_fn: &str, args: &[A]) -> usize {
        let iv = self
            .interp_call(interp_fn, args)
            .unwrap_or_else(|e| panic!("`{}`: interpreter `{interp_fn}` failed: {e}", self.id));
        let wv = self
            .component
            .call_instance(iface, wasm_fn, &self.wasm_args(args))
            .unwrap_or_else(|e| panic!("`{}`: compiled `{wasm_fn}` failed: {e}", self.id))
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("`{}`: `{wasm_fn}` returned no handle", self.id));
        assert!(
            matches!(wv, Val::Resource(_)),
            "`{}`: `{wasm_fn}` should return a resource handle, got {wv:?}",
            self.id
        );
        self.ivals.push(iv);
        self.wvals.push(wv);
        self.ivals.len() - 1
    }
}

/// Convert a wasm result `Val` into the interpreter `Value` it denotes, so
/// both sides print through the same `print_value`. Only the shapes the
/// multi-call scripts actually return are mapped.
fn val_to_value(id: &str, func: &str, v: &Val) -> Value {
    match v {
        Val::Bool(b) => Value::Bool(*b),
        Val::U8(n) => Value::Int(i64::from(*n)),
        Val::U16(n) => Value::Int(i64::from(*n)),
        Val::U32(n) => Value::Int(i64::from(*n)),
        Val::S8(n) => Value::Int(i64::from(*n)),
        Val::S16(n) => Value::Int(i64::from(*n)),
        Val::S32(n) => Value::Int(i64::from(*n)),
        Val::S64(n) => Value::Int(*n),
        Val::String(s) => Value::Str(s.to_string()),
        Val::List(vs) => Value::Lst(vs.iter().map(|x| val_to_value(id, func, x)).collect()),
        other => panic!("`{id}`: `{func}` returned an unmapped value shape: {other:?}"),
    }
}

/// Resource lifecycle across the call boundary: constructor, methods, a
/// static alternative constructor, and two independent live instances — the
/// receiver's cell state must persist between separate host calls exactly as
/// the interpreter's `Rc<RefCell>` env does.
#[test]
fn multicall_counter_state_persists_across_calls() {
    const API: &str = "diff:counter/api@0.1.0";
    let mut m = MultiCall::load("counter");

    let h0 = m.call_handle("counter", API, "[constructor]counter", &[A::U32(3)]);
    // `next` post-increments: 3, 4; then `value` reads 5 twice (no advance).
    m.call("counter/next", API, "[method]counter.next", &[A::H(h0)]);
    m.call("counter/next", API, "[method]counter.next", &[A::H(h0)]);
    m.call("counter/value", API, "[method]counter.value", &[A::H(h0)]);
    m.call("counter/value", API, "[method]counter.value", &[A::H(h0)]);
    // `add-n` folds an argument into the persisted state.
    m.call(
        "counter/add-n",
        API,
        "[method]counter.add-n",
        &[A::H(h0), A::U32(10)],
    );
    m.call("counter/value", API, "[method]counter.value", &[A::H(h0)]);
    // A second instance from the static constructor is independent state.
    let h1 = m.call_handle(
        "counter/sum",
        API,
        "[static]counter.sum",
        &[A::U32List(&[1, 2, 3])],
    );
    m.call("counter/value", API, "[method]counter.value", &[A::H(h1)]);
    m.call("counter/next", API, "[method]counter.next", &[A::H(h0)]);
    m.call("counter/value", API, "[method]counter.value", &[A::H(h1)]);
    m.call("counter/value", API, "[method]counter.value", &[A::H(h0)]);
}

/// Functor-set state across the call boundary: a handle returned from one
/// export call is mutated and queried by later method calls on the SAME live
/// instance — the backend's persisted set state must track the interpreter's
/// shared `SetHandle` per call, in order.
#[test]
fn multicall_set_handle_mutations_persist_across_calls() {
    const API: &str = "diff:sethandle/api@0.1.0";
    const SET: &str = "diff:sethandle/s32-set@0.1.0";
    let mut m = MultiCall::load("set-handle");

    // build-ints() adds 1, 2, 1 (a structural duplicate) and returns the set.
    let h = m.call_handle("build-ints", API, "build-ints", &[]);
    m.call("ints/size", SET, "[method]set.size", &[A::H(h)]);
    m.call(
        "ints/contains",
        SET,
        "[method]set.contains",
        &[A::H(h), A::S32(1)],
    );
    m.call(
        "ints/contains",
        SET,
        "[method]set.contains",
        &[A::H(h), A::S32(9)],
    );
    // Mutate across the boundary, then observe the mutation in later calls.
    m.call("ints/add", SET, "[method]set.add", &[A::H(h), A::S32(5)]);
    m.call("ints/size", SET, "[method]set.size", &[A::H(h)]);
    m.call(
        "ints/contains",
        SET,
        "[method]set.contains",
        &[A::H(h), A::S32(5)],
    );
    // Duplicate add is a no-op on both sides.
    m.call("ints/add", SET, "[method]set.add", &[A::H(h), A::S32(5)]);
    m.call("ints/size", SET, "[method]set.size", &[A::H(h)]);
}
