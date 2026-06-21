pub mod builtins;
pub mod check;
pub mod expand;
pub mod form;
pub mod interp;
pub mod lexer;
// One-step interpreter macro expansion (`expand_one`/`manifest`), retained as
// the differential **oracle** the compiled strategy-B macro components are
// checked against (`tests/macro_differential.rs`). It no longer drives a
// produced component — those compile their bodies to wasm (`emit`,
// `macrobuild`) — but stays non-gated and interpreter-backed for the oracle.
pub mod macrolib;
pub mod printer;
pub mod reader;
pub mod value;

// The compiler back end (emit/componentize/compose) and the fs/stdin-backed
// runner & REPL depend on native-only crates (wasm-encoder, wit-*, wac-graph)
// and so are excluded from the wasm build, which only needs the interpreter.
#[cfg(not(target_arch = "wasm32"))]
pub mod build;
#[cfg(not(target_arch = "wasm32"))]
pub mod emit;
#[cfg(not(target_arch = "wasm32"))]
pub mod host;
#[cfg(not(target_arch = "wasm32"))]
pub mod macrobuild;
#[cfg(not(target_arch = "wasm32"))]
pub mod macrodep;
#[cfg(not(target_arch = "wasm32"))]
pub mod macros;
#[cfg(not(target_arch = "wasm32"))]
pub mod meta;
#[cfg(not(target_arch = "wasm32"))]
pub mod repl;
#[cfg(not(target_arch = "wasm32"))]
pub mod runner;
#[cfg(not(target_arch = "wasm32"))]
pub mod scaffold;
#[cfg(not(target_arch = "wasm32"))]
pub mod tools;
#[cfg(not(target_arch = "wasm32"))]
pub mod wit;
#[cfg(not(target_arch = "wasm32"))]
pub mod witdep;

// Browser playground bindings (compiled only for wasm, and only when the
// `playground` feature is on — the default). Gating the playground bindings
// behind a feature keeps wasm-bindgen out of any other `wasm32` consumer of
// this crate.
#[cfg(all(target_arch = "wasm32", feature = "playground"))]
pub mod wasm;

pub use form::{Arena, Node, NodeId};
pub use lexer::ReadError;
pub use printer::print;
pub use reader::read_file;

use std::cell::RefCell;

thread_local! {
    /// When `Some`, `print`/`println` builtins append here instead of writing
    /// to real stdout. The wasm playground turns this on to capture output;
    /// the native CLI never does, so its behaviour is unchanged.
    static OUTPUT_SINK: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Begin capturing `print`/`println` output into an in-memory buffer.
pub fn output_capture_start() {
    OUTPUT_SINK.with(|s| *s.borrow_mut() = Some(String::new()));
}

/// Stop capturing and return everything written since `output_capture_start`.
pub fn output_capture_take() -> String {
    OUTPUT_SINK.with(|s| s.borrow_mut().take().unwrap_or_default())
}

/// Emit program output: into the capture buffer if active, else real stdout.
pub fn emit_output(text: &str, newline: bool) {
    OUTPUT_SINK.with(|s| {
        let mut slot = s.borrow_mut();
        if let Some(buf) = slot.as_mut() {
            buf.push_str(text);
            if newline {
                buf.push('\n');
            }
        } else if newline {
            println!("{text}");
        } else {
            print!("{text}");
        }
    });
}

/// The result of evaluating a documentation snippet.
#[derive(Debug, Clone)]
pub struct EvalOutcome {
    /// Whether evaluation completed without error.
    pub ok: bool,
    /// Printed value of the final form (empty when it is unit, e.g. a `Def`).
    pub value: String,
    /// Everything `print`/`println` wrote during evaluation.
    pub output: String,
    /// Error message when `ok` is false.
    pub error: String,
}

/// Evaluate a snippet of Wavelet source the way the docs playground and the
/// `wavelet run` interpreter do: install the standard library, read every
/// top-level form, evaluate them in order, and report the final value plus any
/// captured output. This is the single evaluation path shared by the wasm
/// bindings ([`wasm`]) and the documentation-example test suite, so a language
/// change that breaks a documented example breaks `cargo test`.
pub fn eval_snippet(src: &str) -> EvalOutcome {
    use std::rc::Rc;
    use value::{print_value, unit, Env, Value};

    let (arena, roots) = match read_file(src) {
        Ok(pair) => pair,
        Err(e) => {
            return EvalOutcome {
                ok: false,
                value: String::new(),
                output: String::new(),
                error: e.to_string(),
            };
        }
    };
    // Static type checking + overload resolution run before evaluation: an
    // ill-typed (or ambiguously-overloaded) program is a compile error even
    // when the bad code is never reached at runtime. `resolve_overloads` checks
    // the program, then rewrites overload sets to uniquely-named defs so the
    // interpreter sees no overloading. With no overload set it is an identity.
    let (arena, roots) = match check::resolve_overloads(arena, &roots) {
        Ok(pair) => pair,
        Err(msg) => {
            return EvalOutcome {
                ok: false,
                value: String::new(),
                output: String::new(),
                error: msg,
            };
        }
    };
    let arena = Rc::new(arena);

    let interp = interp::Interp::new();
    let env = Env::root();
    builtins::install(&env);

    output_capture_start();
    let mut last = unit();
    for root in roots {
        match interp.eval(&arena, root, &env) {
            Ok(v) => last = v,
            Err(e) => {
                return EvalOutcome {
                    ok: false,
                    value: String::new(),
                    output: output_capture_take(),
                    error: e.to_string(),
                };
            }
        }
    }
    let output = output_capture_take();

    // Suppress a trailing unit so the playground doesn't show a noisy `{}`.
    let value = if matches!(&last, Value::Rec(f) if f.is_empty()) {
        String::new()
    } else {
        print_value(&last)
    };
    EvalOutcome {
        ok: true,
        value,
        output,
        error: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read1(src: &str) -> String {
        let (arena, roots) = read_file(src).expect(src);
        assert_eq!(roots.len(), 1, "expected one form in {src:?}");
        print(&arena, roots[0])
    }

    fn read_all(src: &str) -> Vec<String> {
        let (arena, roots) = read_file(src).expect(src);
        roots.iter().map(|&r| print(&arena, r)).collect()
    }

    #[test]
    fn atoms() {
        assert_eq!(read1("true"), "true");
        assert_eq!(read1("42"), "42");
        assert_eq!(read1("-1.5"), "-1.5");
        assert_eq!(read1("6.022e+23"), "6.022e23");
        assert_eq!(read1("-inf"), "-inf");
        assert_eq!(read1("nan"), "nan");
        assert_eq!(read1("'x'"), "'x'");
        assert_eq!(read1("'☃'"), "'☃'");
        assert_eq!(read1(r#"'\u{0}'"#), r#"'\u{0}'"#);
        assert_eq!(read1(r#""abc\t123""#), r#""abc\t123""#);
    }

    #[test]
    fn desugar_table() {
        assert_eq!(read1("foo"), "foo");
        assert_eq!(read1("f(x)"), "(f, x)");
        assert_eq!(read1("f(x y)"), "(f, x, y)");
        assert_eq!(read1("f([x y])"), "(f, [x, y])");
        assert_eq!(read1("f({a: 1 b: 2})"), "(f, {a: 1, b: 2})");
        assert_eq!(read1("f()"), "(f)");
        assert_eq!(read1("kv/get({bucket: b})"), "(kv/get, {bucket: b})");
        assert_eq!(read1("(a b)"), "(a, b)");
        assert_eq!(read1("(a)"), "(a)");
        assert_eq!(read1("[a b]"), "[a, b]");
        assert_eq!(read1("{k: v}"), "{k: v}");
        assert_eq!(read1("{read write}"), "{read, write}");
        assert_eq!(read1("{}"), "{}");
        assert_eq!(read1("If c t e"), "(if-MACRO, c, t, e)");
        assert_eq!(read1("Unquote(x)"), "(unquote-MACRO, x)");
        // the list/record call sugar was removed — `[`/`{` no longer attach
        assert!(read_file("f[x y]").is_err());
        assert!(read_file("f{a: 1 b: 2}").is_err());
    }

    #[test]
    fn commas_are_whitespace() {
        assert_eq!(read1("[1, 2, 3]"), read1("[1 2 3]"));
        assert_eq!(read1("f(x, y)"), "(f, x, y)");
    }

    #[test]
    fn attachment_rule() {
        // attaching `{` to a name is a read error now
        assert!(read_file(r#"delete-file{path: "foo.md" force: true}"#).is_err());
        // with whitespace, the name and the record are two separate forms
        assert_eq!(
            read_all(r#"delete-file {path: "foo.md" force: true}"#),
            vec!["delete-file".to_string(), r#"{path: "foo.md", force: true}"#.to_string()]
        );
    }

    #[test]
    fn macro_arity_reading() {
        assert_eq!(read1("Quote foo"), "(quote-MACRO, foo)");
        assert_eq!(
            read1(r#"If eq(foo bar) say("match") say("nope")"#),
            r#"(if-MACRO, (eq, foo, bar), (say, "match"), (say, "nope"))"#
        );
        // nested macro forms need no delimiters
        assert_eq!(
            read1("Def run Fn {} If c a b"),
            "(def-MACRO, run, (fn-MACRO, {}, (if-MACRO, c, a, b)))"
        );
        // explicit payload overrides arity reading
        assert_eq!(read1("If(c t e)"), "(if-MACRO, c, t, e)");
    }

    #[test]
    fn def_macro_registers_arity() {
        let forms = read_all(
            "DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false\n\
             And lt(x 10) gt(x 0)",
        );
        assert_eq!(forms.len(), 2);
        assert_eq!(forms[1], "(and-MACRO, (lt, x, 10), (gt, x, 0))");
    }

    #[test]
    fn unknown_macro_is_an_error() {
        assert!(read_file("Bogus 1 2").is_err());
    }

    #[test]
    fn title_case_does_not_collide_with_upper_words() {
        // A lower-first or all-UPPER-first kebab word is an ordinary identifier,
        // never a macro head — TitleCase needs a Title-case *leading* word.
        assert_eq!(read1("parse-JSON(x)"), "(parse-JSON, x)");
        assert_eq!(read1("HTTP-get(x)"), "(HTTP-get, x)");
        // TitleCase heads (not core forms) need an explicit payload here. Both a
        // single word and a hyphenated head lower-case whole onto a `-MACRO` name.
        assert_eq!(read1("TryLet({a: b} c)"), "(trylet-MACRO, {a: b}, c)");
        assert_eq!(read1("Try-let({a: b} c)"), "(try-let-MACRO, {a: b}, c)");
    }

    #[test]
    fn call_chaining() {
        // a receiver becomes the first argument of the chained call (§2.5)
        assert_eq!(read1("1.increment()"), "(increment, 1)");
        assert_eq!(read1("x.foo(a b)"), "(foo, x, a, b)");
        // chains nest left-to-right
        assert_eq!(
            read1("foo(1 2 3).bar(4 5 6).baz(7 8 9)"),
            "(baz, (bar, (foo, 1, 2, 3), 4, 5, 6), 7, 8, 9)"
        );
        // the receiver can be any primary form
        assert_eq!(read1("1.5.to-string()"), "(to-string, 1.5)");
        assert_eq!(read1("[1 2].len()"), "(len, [1, 2])");
        assert_eq!(read1(r#""hi".upper()"#), r#"(upper, "hi")"#);
        // a qualified name works as the chained call head
        assert_eq!(read1("b.kv/get(k)"), "(kv/get, b, k)");
        // whitespace breaks attachment: `.` must abut the receiver and the name
        assert!(read_file("1 .increment()").is_err());
        assert!(read_file("1. increment()").is_err());
        // a bare `.name` with no `(...)` is not (yet) field access
        assert!(read_file("foo.bar").is_err());
    }

    #[test]
    fn comments_and_newlines() {
        assert_eq!(
            read1("// leading comment\nf(x) // trailing"),
            "(f, x)"
        );
    }

    #[test]
    fn match_clause_shape() {
        assert_eq!(
            read1("Match r [ (ok(text) process(text)) (err(e) handle(e)) ]"),
            "(match-MACRO, r, [((ok, text), (process, text)), ((err, e), (handle, e))])"
        );
    }

    fn eval_str(src: &str) -> String {
        let (arena, roots) = read_file(src).expect(src);
        let arena = std::rc::Rc::new(arena);
        let env = value::Env::root();
        builtins::install(&env);
        let env = env.child();
        let interp = interp::Interp::new();
        let mut last = value::unit();
        for root in roots {
            last = interp.eval(&arena, root, &env).expect(src);
        }
        value::print_value(&last)
    }

    #[test]
    fn eval_expand_builtin() {
        let src = "DefMacro unless {c e} Quasi If Unquote(c) {} Unquote(e)\n\
                   expand(Quote Unless(false \"ran\"))";
        assert_eq!(eval_str(src), "(if-MACRO, false, {}, \"ran\")");
        // non-macro forms pass through one step of expand unchanged
        assert_eq!(eval_str("expand(Quote add(1 2))"), "(add, 1, 2)");
        assert_eq!(eval_str("expand(42)"), "42");
    }

    #[test]
    fn eval_nested_quasi_depth() {
        // a nested Quasi protects its Unquotes; Unquote(Unquote(x)) fires the
        // innermost one level down
        assert_eq!(
            eval_str("Quasi [Unquote(add(1 2)) Quasi Unquote(add(1 2)) Quasi Unquote(Unquote(add(1 2)))]"),
            "[3, (quasi-MACRO, (unquote-MACRO, (add, 1, 2))), (quasi-MACRO, (unquote-MACRO, 3))]"
        );
    }

    #[test]
    fn eval_atoms_and_calls() {
        assert_eq!(eval_str("add(1 2)"), "3");
        assert_eq!(eval_str("str-cat(upper(\"wasm\") \"!\")"), "\"WASM!\"");
        assert_eq!(eval_str("eq(Quote (1 2) Quote (1 2))"), "true");
        assert_eq!(eval_str("If lt(1 2) \"yes\" \"no\""), "\"yes\"");
    }

    #[test]
    fn eval_def_fn_and_payload_binding() {
        // record payload binds by name, positional args by order (§4.2)
        let src = "Def f Fn {path force} [path force]";
        assert_eq!(eval_str(&format!("{src} f({{path: \"a\" force: true}})")), "[\"a\", true]");
        assert_eq!(eval_str(&format!("{src} f(\"a\" true)")), "[\"a\", true]");
        // a sole parameter receives the bundled payload directly
        assert_eq!(eval_str("Def id Fn {x} x id([1 2 3])"), "[1, 2, 3]");
        assert_eq!(eval_str("head([1 2 3])"), "1");
    }

    #[test]
    fn eval_typed_params() {
        assert_eq!(eval_str("Def s Fn {phrase: string} upper(phrase) s(\"hi\")"), "\"HI\"");
        let (arena, roots) = read_file("Def s Fn {phrase: string} phrase s(42)").unwrap();
        let arena = std::rc::Rc::new(arena);
        let env = value::Env::root();
        builtins::install(&env);
        let interp = interp::Interp::new();
        let mut result = Ok(value::unit());
        for root in roots {
            result = interp.eval(&arena, root, &env);
            if result.is_err() {
                break;
            }
        }
        assert!(result.is_err(), "type check should reject 42 for string");
    }

    #[test]
    fn eval_let_do_match() {
        assert_eq!(eval_str("Let {x: 2 y: mul(x 3)} add(x y)"), "8");
        assert_eq!(eval_str("Do [drop(\"\") 7]"), "7");
        assert_eq!(
            eval_str("Match ok(5) [ (ok(n) add(n 1)) (err(e) 0) ]"),
            "6"
        );
        assert_eq!(
            eval_str("Match err(\"boom\") [ (ok(n) n) (err(e) e) ]"),
            "\"boom\""
        );
        assert_eq!(eval_str("Match none [ (none 1) (some(x) x) ]"), "1");
        assert_eq!(eval_str("Match some(9) [ (none 1) (some(x) x) ]"), "9");
    }

    #[test]
    fn eval_tail_recursion_constant_stack() {
        assert_eq!(
            eval_str(
                "Def count-down Fn {n} If eq(n 0) \"liftoff\" count-down(sub(n 1))\n\
                 count-down(200000)"
            ),
            "\"liftoff\""
        );
    }

    #[test]
    fn eval_closures_capture() {
        assert_eq!(
            eval_str("Def make Fn {n} Fn {m} add(n m) Def add5 make(5) add5(3)"),
            "8"
        );
        assert_eq!(eval_str("map(Fn {x} mul(x x) [1 2 3])"), "[1, 4, 9]");
    }

    #[test]
    fn eval_quote_quasi_macro() {
        assert_eq!(eval_str("Quote add(1 2)"), "(add, 1, 2)");
        assert_eq!(eval_str("Let {x: 2} Quasi add(1 Unquote(x))"), "(add, 1, 2)");
        assert_eq!(eval_str("Quasi [1 Splice([2 3]) 4]"), "[1, 2, 3, 4]");
        assert_eq!(
            eval_str(
                "DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false\n\
                 And lt(1 2) lt(2 3)"
            ),
            "true"
        );
        assert_eq!(
            eval_str(
                "DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false\n\
                 And lt(2 1) boom-unbound(1)"
            ),
            "false"
        );
    }

    #[test]
    fn eval_try_let_macro_from_spec() {
        // §7.2: error propagation as a binding form, in user space
        let src = "\
DefMacro try-let {binding body}
  Let {name: rec-key(binding) expr: rec-val(binding)}
    Quasi Match Unquote(expr) [
      (ok(Unquote(name))  Unquote(body))
      (err(e)             err(e))
    ]
Def half Fn {n} If eq(rem(n 2) 0) ok(div(n 2)) err(\"odd\")
Def quarter Fn {n}
  Try-let {h: half(n)}
  Try-let {q: half(h)}
  ok(q)
[quarter(12) quarter(6)]";
        assert_eq!(eval_str(src), "[ok(3), err(\"odd\")]");
    }

    #[test]
    fn eval_cells_and_misc() {
        assert_eq!(eval_str("Let {c: cell-new(1)} Do [cell-set(c 5) cell-get(c)]"), "5");
        assert_eq!(eval_str("fold(Fn {a b} add(a b) 0 range(1 5))"), "10");
        assert_eq!(eval_str("to-string({a: 1})"), "\"{a: 1}\"");
        assert_eq!(eval_str("read(\"ok(5)\")"), "ok((ok, 5))");
    }

    #[test]
    fn wit_synthesis_matches_spec() {
        // §6.1: the exact WIT the design doc shows for shout.wvl
        let src = "Package \"demo:shout@0.1.0\"\n\
                   Export shout\n\
                   Def shout Fn {phrase: string}\n\
                     str-cat(upper(phrase) \"!\")";
        let (arena, roots) = read_file(src).unwrap();
        let got = wit::synthesize(&arena, &roots).unwrap();
        let want = "\
package demo:shout@0.1.0;

interface api {
  shout: func(phrase: string) -> string;
}

world shout {
  export api;
}
";
        assert_eq!(got, want);
    }

    #[test]
    fn wit_synthesis_world_imports() {
        let src = "Package \"demo:main@0.1.0\"\n\
                   Import {pkg: \"demo:shout/api\" as: sh}\n\
                   Export run\n\
                   Def run Fn {} drop(\"hi\")";
        let (arena, roots) = read_file(src).unwrap();
        let got = wit::synthesize(&arena, &roots).unwrap();
        assert!(got.contains("run: func();"), "{got}");
        assert!(got.contains("import demo:shout/api;"), "{got}");
    }

    #[test]
    fn wit_synthesis_types_and_explicit_exports() {
        let src = "Package \"demo:t@0.1.0\"\n\
                   DefType pair {x: s32 y: s32}\n\
                   DefType ttl [days(u32) forever]\n\
                   Export {name: pick params: {seed: u64} result: result(string string)}\n\
                   Def pick Fn {seed} ok(\"w\")";
        let (arena, roots) = read_file(src).unwrap();
        let got = wit::synthesize(&arena, &roots).unwrap();
        assert!(got.contains("record pair { x: s32, y: s32 }"), "{got}");
        assert!(got.contains("variant ttl { days(u32), forever }"), "{got}");
        assert!(
            got.contains("pick: func(seed: u64) -> result<string, string>;"),
            "{got}"
        );
    }

    #[test]
    fn emit_match_componentizes() {
        let src = "Package \"demo:matchy@0.1.0\"\n\
                   Export run\n\
                   Def run Fn {}\n\
                     Match [\"wasm\"] [\n\
                       ([\"wasm\"] \"WASM!\")\n\
                       ([w] str-cat(\"got \" w))\n\
                       (other \"usage\")]";
        let (arena, roots) = read_file(src).unwrap();
        let info = wit::collect(&arena, &roots).unwrap();
        let bytes =
            emit::emit_component(&arena, &roots, &info, &std::collections::HashMap::new())
                .unwrap();
        assert!(bytes.starts_with(b"\0asm"));
    }

    #[test]
    fn emit_closures_componentize() {
        // anonymous Fn with capture, higher-order param, named def as value,
        // module-level value def holding a closure, to-string of ints
        let src = "Package \"demo:clo@0.1.0\"\n\
                   Export run\n\
                   Def make-adder Fn {n: s64}\n\
                     Fn {m: s64} add(n m)\n\
                   Def twice Fn {f x} f(f(x))\n\
                   Def inc Fn {n: s64} add(n 1)\n\
                   Def add5 make-adder(5)\n\
                   Def run Fn {}\n\
                     Do [\n\
                       to-string(add5(3))\n\
                       to-string(twice(add5 10))\n\
                       to-string(twice(inc neg(1)))]";
        let (arena, roots) = read_file(src).unwrap();
        let info = wit::collect(&arena, &roots).unwrap();
        let bytes =
            emit::emit_component(&arena, &roots, &info, &std::collections::HashMap::new())
                .unwrap();
        assert!(bytes.starts_with(b"\0asm"));
    }

    #[test]
    fn emit_value_defs_and_list_literals() {
        let src = "Package \"demo:vals@0.1.0\"\n\
                   Def greeting str-cat(\"hello\" \", world\")\n\
                   Export run\n\
                   Def run Fn {}\n\
                     Do [\n\
                       greeting\n\
                       Match [\"a\" \"b\"] [\n\
                         ([\"a\" x] str-cat(x))\n\
                         (other \"no\")]]";
        let (arena, roots) = read_file(src).unwrap();
        let info = wit::collect(&arena, &roots).unwrap();
        let bytes =
            emit::emit_component(&arena, &roots, &info, &std::collections::HashMap::new())
                .unwrap();
        assert!(bytes.starts_with(b"\0asm"));
    }

    #[test]
    fn emit_records_componentize() {
        // record literal construction + record-pattern Match (subset of fields);
        // run's result is inferred through Let/Match to Unit
        let src = "Package \"demo:rec@0.1.0\"\n\
                   Export run\n\
                   Def run Fn {}\n\
                     Let {p: {x: 3 y: 7 label: \"pt\"}}\n\
                       Match p [\n\
                         ({x: a label: l} str-cat(l to-string(a)))\n\
                         (other \"no\")]";
        let (arena, roots) = read_file(src).unwrap();
        let info = wit::collect(&arena, &roots).unwrap();
        let bytes =
            emit::emit_component(&arena, &roots, &info, &std::collections::HashMap::new())
                .unwrap();
        assert!(bytes.starts_with(b"\0asm"));
    }

    #[test]
    fn eval_record_construct_and_match() {
        // the interpreter and wasm backend agree on this program's result
        assert_eq!(
            eval_str("Let {p: {x: 3 y: 7 label: \"pt\"}}\n\
                      Match p [({x: a y: b label: l} l) (other \"no\")]"),
            "\"pt\""
        );
    }

    #[test]
    fn emit_variants_and_tuples_componentize() {
        // variant constructors (some/ok/err/none) and variant patterns, plus a
        // tuple-payload variant: `some(1 2)` bundles its two args into a tuple
        // payload, exercising tuple construction in the wasm backend
        let src = "Package \"demo:var@0.1.0\"\n\
                   Def describe Fn {r}\n\
                     Match r [\n\
                       (ok(n) to-string(n))\n\
                       (err(e) e)\n\
                       (none \"nothing\")\n\
                       (some(x) to-string(x))]\n\
                   Export run\n\
                   Def run Fn {}\n\
                     Do [\n\
                       describe(ok(42))\n\
                       describe(none)\n\
                       to-string(some(1 2))]";
        let (arena, roots) = read_file(src).unwrap();
        let info = wit::collect(&arena, &roots).unwrap();
        let bytes =
            emit::emit_component(&arena, &roots, &info, &std::collections::HashMap::new())
                .unwrap();
        assert!(bytes.starts_with(b"\0asm"));
    }

    #[test]
    fn eval_variant_and_tuple_match() {
        // interpreter reference for the wasm-backend behavior above
        assert_eq!(
            eval_str("Match ok(42) [(ok(n) n) (err(e) 0)]"),
            "42"
        );
        assert_eq!(eval_str("Match none [(none \"y\") (some(x) \"n\")]"), "\"y\"");
        // a tuple value (from Quote) destructures element-wise
        assert_eq!(eval_str("Match Quote (10 20) [((a b) add(a b)) (other 0)]"), "30");
    }

    #[test]
    fn triple_slash_is_an_ordinary_comment() {
        // `///` is no longer a doc comment: it is discarded like any `//`
        // comment and never reaches the form tree or the synthesized WIT.
        let src = "Package \"demo:doc@0.1.0\"\n\
                   /// A pair.\n\
                   DefType point {x: s32 y: s32}\n\
                   /// Shouts.\n\
                   /// Loudly.\n\
                   Export shout\n\
                   Def shout Fn {phrase: string} upper(phrase)";
        let (arena, roots) = read_file(src).unwrap();
        // The comments leave no tokens, so the roots are exactly the four forms.
        assert_eq!(roots.len(), 4);
        let got = wit::synthesize(&arena, &roots).unwrap();
        assert!(!got.contains("///"), "{got}");
    }

    #[test]
    fn wit_grouped_exports() {
        let src = "Package \"demo:gfx@0.1.0\"\n\
                   Export {iface: \"render\" name: frame}\n\
                   Def frame Fn {label: string} str-cat(\"<\" label \">\")\n\
                   Export ping\n\
                   Def ping Fn {} \"pong\"";
        let (arena, roots) = read_file(src).unwrap();
        let got = wit::synthesize(&arena, &roots).unwrap();
        assert!(got.contains("interface render {\n  frame: func(label: string) -> string;"), "{got}");
        assert!(got.contains("interface api {\n  ping: func() -> string;"), "{got}");
        assert!(got.contains("export render;"), "{got}");
        assert!(got.contains("export api;"), "{got}");
    }

    #[test]
    fn wit_inference_follows_calls_to_other_defs() {
        let src = "Package \"demo:i@0.1.0\"\n\
                   Export double\n\
                   Def double Fn {n: s64} helper(n)\n\
                   Def helper Fn {x: s64} mul(x x)\n\
                   Export greet\n\
                   Def greet Fn {name: string} shout(name)\n\
                   Def shout Fn {s: string} upper(s)\n\
                   Def loop-y Fn {n: s64} loop-y(n)";
        let (arena, roots) = read_file(src).unwrap();
        let got = wit::synthesize(&arena, &roots).unwrap();
        assert!(got.contains("double: func(n: s64) -> s64;"), "{got}");
        assert!(got.contains("greet: func(name: string) -> string;"), "{got}");
    }

    #[test]
    fn round_trip_is_stable() {
        for src in [
            "(f, x, y)",
            "(if-MACRO, c, t, e)",
            r#"(delete-file, {path: "foo.md", force: true})"#,
            "[1, 2, [3, (a, b)], {f: {read, write}}]",
        ] {
            assert_eq!(read1(src), src, "canonical text must read back unchanged");
        }
    }

    #[test]
    fn emit_components_for_spec_demo() {
        // shout.wvl: no deps, exports api#shout
        let (sa, sr) = read_file(include_str!("../examples/shout.wvl")).unwrap();
        let sinfo = wit::collect(&sa, &sr).unwrap();
        let bytes = emit::emit_component(&sa, &sr, &sinfo, &Default::default())
            .expect("shout componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");

        // main.wvl: imports demo:shout/api, exports run
        let (ma, mr) = read_file(include_str!("../examples/main.wvl")).unwrap();
        let minfo = wit::collect(&ma, &mr).unwrap();
        let mut deps = std::collections::HashMap::new();
        deps.insert(
            "demo:shout".to_string(),
            emit::Dep {
                package: sinfo.package.clone(),
                funcs: sinfo.exports.clone(),
                package_wit: emit::dep_package_wit(&sa, &sinfo).unwrap(),
                types: emit::dep_record_types(&sa, &sinfo),
                type_defs: Vec::new(),
            },
        );
        let bytes = emit::emit_component(&ma, &mr, &minfo, &deps)
            .expect("main componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn emit_lists_across_boundaries() {
        // provider: list<string> param and result, list<s64> param
        let psrc = "Package \"demo:lists@0.1.0\"\n\
                    Export {name: first-up params: {words: list(string)} result: string}\n\
                    Def first-up Fn {words}\n\
                      If gt(len(words) 0) upper(head(words)) \"EMPTY\"\n\
                    Export {name: echo params: {words: list(string)} result: list(string)}\n\
                    Def echo Fn {words} words\n\
                    Export {name: sum3 params: {ns: list(s64)} result: s64}\n\
                    Def sum3 Fn {ns}\n\
                      Match ns [([a b c] add(a add(b c))) (other 0)]";
        let (pa, pr) = read_file(psrc).unwrap();
        let pinfo = wit::collect(&pa, &pr).unwrap();
        let bytes = emit::emit_component(&pa, &pr, &pinfo, &Default::default())
            .expect("list provider componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");

        let msrc = "Package \"demo:listmain@0.1.0\"\n\
                     Import {pkg: \"demo:lists/api\" as: lst}\n\
                    Export run\n\
                    Def run Fn {}\n\
                      Do [\n\
                        lst/first-up({words: [\"hello\"]})\n\
                        head(lst/echo({words: [\"a\" \"b\"]}))\n\
                        to-string(lst/sum3({ns: [10 20 12]}))]";
        let (ma, mr) = read_file(msrc).unwrap();
        let minfo = wit::collect(&ma, &mr).unwrap();
        let mut deps = std::collections::HashMap::new();
        deps.insert(
            "demo:lists".to_string(),
            emit::Dep {
                package: pinfo.package.clone(),
                funcs: pinfo.exports.clone(),
                package_wit: emit::dep_package_wit(&pa, &pinfo).unwrap(),
                types: emit::dep_record_types(&pa, &pinfo),
                type_defs: Vec::new(),
            },
        );
        let bytes = emit::emit_component(&ma, &mr, &minfo, &deps)
            .expect("list consumer componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn emit_records_across_boundaries() {
        // provider: returns a record (retptr w/ field layout) and takes one
        // (flattened params); mixed scalar widths exercise alignment
        let psrc = "Package \"demo:geo@0.1.0\"\n\
                    DefType point {x: s64 y: s64}\n\
                    Export {name: make-point params: {x: s64 y: s64} result: point}\n\
                    Def make-point Fn {x: s64 y: s64} {x: x y: y}\n\
                    Export {name: sum-coords params: {p: point} result: s64}\n\
                    Def sum-coords Fn {p}\n\
                      Match p [({x: a y: b} add(a b)) (other 0)]";
        let (pa, pr) = read_file(psrc).unwrap();
        let pinfo = wit::collect(&pa, &pr).unwrap();
        // the synthesized WIT (record decl, no trailing `;`) must parse + encode
        let bytes = emit::emit_component(&pa, &pr, &pinfo, &Default::default())
            .expect("record provider componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");

        let msrc = "Package \"demo:geomain@0.1.0\"\n\
                     Import {pkg: \"demo:geo/api\" as: g}\n\
                    Export run\n\
                    Def run Fn {}\n\
                      Let {p: g/make-point({x: 3 y: 39})}\n\
                        to-string(g/sum-coords({p: p}))";
        let (ma, mr) = read_file(msrc).unwrap();
        let minfo = wit::collect(&ma, &mr).unwrap();
        let mut deps = std::collections::HashMap::new();
        deps.insert(
            "demo:geo".to_string(),
            emit::Dep {
                package: pinfo.package.clone(),
                funcs: pinfo.exports.clone(),
                package_wit: emit::dep_package_wit(&pa, &pinfo).unwrap(),
                types: emit::dep_record_types(&pa, &pinfo),
                type_defs: Vec::new(),
            },
        );
        let bytes = emit::emit_component(&ma, &mr, &minfo, &deps)
            .expect("record consumer componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn emit_option_result_across_boundaries() {
        // provider returns option<s64> and result<s64, string> (the latter has
        // differing arm flat shapes, exercising the in-memory variant path)
        let psrc = "Package \"demo:opt@0.1.0\"\n\
                    Export {name: lookup params: {k: s64} result: option(s64)}\n\
                    Def lookup Fn {k} If gt(k 0) some(mul(k 10)) none\n\
                    Export {name: checked params: {k: s64} result: result(s64 string)}\n\
                    Def checked Fn {k} If gt(k 0) ok(k) err(\"nonpositive\")";
        let (pa, pr) = read_file(psrc).unwrap();
        let pinfo = wit::collect(&pa, &pr).unwrap();
        let bytes = emit::emit_component(&pa, &pr, &pinfo, &Default::default())
            .expect("option/result provider componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");

        let msrc = "Package \"demo:optmain@0.1.0\"\n\
                     Import {pkg: \"demo:opt/api\" as: o}\n\
                    Export run\n\
                    Def run Fn {}\n\
                      Do [\n\
                        Match o/lookup({k: 4}) [(some(v) to-string(v)) (none \"none\")]\n\
                        Match o/checked({k: 0}) [(ok(v) to-string(v)) (err(e) str-cat(e))]]";
        let (ma, mr) = read_file(msrc).unwrap();
        let minfo = wit::collect(&ma, &mr).unwrap();
        let mut deps = std::collections::HashMap::new();
        deps.insert(
            "demo:opt".to_string(),
            emit::Dep {
                package: pinfo.package.clone(),
                funcs: pinfo.exports.clone(),
                package_wit: emit::dep_package_wit(&pa, &pinfo).unwrap(),
                types: emit::dep_record_types(&pa, &pinfo),
                type_defs: Vec::new(),
            },
        );
        let bytes = emit::emit_component(&ma, &mr, &minfo, &deps)
            .expect("option/result consumer componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn emit_list_of_records_across_boundaries() {
        // list<record>: element marshalling delegates to store_to_mem/
        // load_from_mem, so list elements may themselves be aggregates
        let psrc = "Package \"demo:lr2@0.1.0\"\n\
                    DefType pt {x: s64 y: s64}\n\
                    Export {name: pts params: {n: s64} result: list(pt)}\n\
                    Def pts Fn {n} [{x: n y: mul(n 2)} {x: add(n 1) y: 0}]";
        let (pa, pr) = read_file(psrc).unwrap();
        let pinfo = wit::collect(&pa, &pr).unwrap();
        let bytes = emit::emit_component(&pa, &pr, &pinfo, &Default::default())
            .expect("list<record> provider componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn emit_list_fields_in_aggregates_across_boundaries() {
        // a record with a list field, and option<list<s64>>, both crossing the
        // boundary (list in memory is (ptr, len) with a canonical element buffer)
        let psrc = "Package \"demo:lrec@0.1.0\"\n\
                    DefType bag {tag: string items: list(s64)}\n\
                    Export {name: mk params: {tag: string items: list(s64)} result: bag}\n\
                    Def mk Fn {tag: string items: list(s64)} {tag: tag items: items}\n\
                    Export {name: maybe params: {k: s64} result: option(list(s64))}\n\
                    Def maybe Fn {k} If gt(k 0) some([k mul(k 2)]) none";
        let (pa, pr) = read_file(psrc).unwrap();
        let pinfo = wit::collect(&pa, &pr).unwrap();
        let bytes = emit::emit_component(&pa, &pr, &pinfo, &Default::default())
            .expect("list-aggregate provider componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn aot_expansion_feeds_the_wasm_backend() {
        let src = r#"
            Package "demo:twice@0.1.0"
            DefMacro twice {x} Quasi mul(2 Unquote(x))
            Export double
            Def double Fn {n: s64} Twice(n)
        "#;
        let (arena, roots) = read_file(src).unwrap();
        let (arena, roots) = expand::expand_file(arena, &roots, None).unwrap();
        // the DefMacro form is gone and the call site is rewritten
        let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
        assert!(printed.iter().all(|s| !s.contains("def-macro")));
        assert!(printed.iter().any(|s| s.contains("(mul, 2, n)")), "{printed:?}");
        // and the expanded tree compiles to a component
        let info = wit::collect(&arena, &roots).unwrap();
        let bytes = emit::emit_component(&arena, &roots, &info, &Default::default())
            .expect("expanded file componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    // -- Step 7: foreign macros expand through their component, to fixpoint -----

    /// Absolute path to the checked-in fixture macro component (identity/1,
    /// unless/2, boom/0). Used by the foreign-expand end-to-end tests.
    fn fixture_macros_path() -> String {
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/macros.wasm").to_string()
    }

    /// Read a file with foreign-macro registration, then run the ahead-of-time
    /// expander with the foreign-expand capability wired in (exactly as
    /// `wavelet build` does). Returns the expanded `(arena, roots)`.
    fn expand_with_foreign(src: &str) -> (Arena, Vec<form::NodeId>) {
        let root = env!("CARGO_MANIFEST_DIR");
        let (arena, roots) =
            macrodep::read_file_with_macros(src, root).expect("read with foreign macros");
        let mut foreign = macrodep::FileExpander::for_file(root, &arena, &roots);
        expand::expand_file(
            arena,
            &roots,
            foreign
                .as_mut()
                .map(|f| f as &mut dyn expand::ForeignExpander),
        )
        .expect("expand with foreign macros")
    }

    /// A source file that imports the fixture macro library (by absolute
    /// `from:` path) and then uses one of its macros.
    fn src_using_fixture(tail: &str) -> String {
        format!(
            "Package \"demo:app@0.1.0\"\n\
             Import {{pkg: \"acme:html/dsl\" macros: true from: \"{}\"}}\n\
             {tail}\n",
            fixture_macros_path()
        )
    }

    #[test]
    fn foreign_macro_expands_and_splices_to_fixpoint() {
        // `unless(c body)` -> `(if-MACRO c {} body)`. The args (`gt(n 0)`, `42`)
        // are spliced into the expansion, and the loop recurses *into* the
        // result — proving the foreign path keeps expanding exactly like a local
        // macro. `if-MACRO` is a core special form (not a user macro), so it is
        // the correct terminal form and survives; were it a user macro it would
        // expand too.
        let src = src_using_fixture(
            "Export run\n\
             Def run Fn {n: s64} Unless gt(n 0) 42",
        );
        let (arena, roots) = expand_with_foreign(&src);
        let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
        assert!(
            printed.iter().all(|s| !s.contains("unless-MACRO")),
            "unless should be expanded away: {printed:?}"
        );
        assert!(
            printed.iter().any(|s| s.contains("(if-MACRO, (gt, n, 0), {}, 42)")),
            "unless must expand into its If form, args spliced in: {printed:?}"
        );
    }

    #[test]
    fn foreign_macro_expansion_is_re_expanded_to_fixpoint() {
        // Prove the loop re-expands a foreign result that *itself* contains a
        // macro call: feed `unless`'s body another foreign macro use
        // (`Identity add(n 1)`). After `unless` expands, the `(identity ...)`
        // sitting in the body must ALSO be expanded — leaving no foreign head.
        let src = src_using_fixture(
            "Export run\n\
             Def run Fn {n: s64} Unless gt(n 0) Identity add(n 1)",
        );
        let (arena, roots) = expand_with_foreign(&src);
        let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
        assert!(
            printed
                .iter()
                .all(|s| !s.contains("unless-MACRO") && !s.contains("identity-MACRO")),
            "no foreign macro head may survive the fixpoint: {printed:?}"
        );
        // The nested `identity(add(n 1))` collapsed to `(add, n, 1)` inside the If.
        assert!(
            printed
                .iter()
                .any(|s| s.contains("(if-MACRO, (gt, n, 0), {}, (add, n, 1))")),
            "nested foreign macro in unless's body must be expanded too: {printed:?}"
        );
    }

    #[test]
    fn foreign_identity_macro_expands_and_componentizes() {
        // `identity(x)` -> `x`: a single-step foreign expansion whose result is
        // emittable, so the foreign-expanded file componentizes end-to-end (the
        // keystone payoff).
        let src = src_using_fixture(
            "Export {name: run params: {n: s64} result: s64}\n\
             Def run Fn {n: s64} Identity add(n 1)",
        );
        let (arena, roots) = expand_with_foreign(&src);
        let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
        assert!(
            printed.iter().any(|s| s.contains("(add, n, 1)")),
            "identity should yield its argument unchanged: {printed:?}"
        );
        assert!(
            printed.iter().all(|s| !s.contains("identity-MACRO")),
            "identity head should be gone: {printed:?}"
        );
        let info = wit::collect(&arena, &roots).expect("collect");
        let bytes = emit::emit_component(&arena, &roots, &info, &Default::default())
            .expect("foreign-expanded file componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn foreign_macro_error_surfaces_with_macro_name() {
        // `boom` always returns `result::err`; the failure must surface as an
        // actionable expand error naming the macro.
        let src = src_using_fixture("Boom");
        let root = env!("CARGO_MANIFEST_DIR");
        let (arena, roots) =
            macrodep::read_file_with_macros(&src, root).expect("read");
        let mut foreign = macrodep::FileExpander::for_file(root, &arena, &roots);
        let err = expand::expand_file(
            arena,
            &roots,
            foreign
                .as_mut()
                .map(|f| f as &mut dyn expand::ForeignExpander),
        )
        .expect_err("boom must fail expansion");
        assert!(err.contains("boom"), "error should name the macro: {err}");
        assert!(
            err.contains("expanding"),
            "error should be framed as an expansion failure: {err}"
        );
    }

    #[test]
    fn foreign_macro_under_quote_is_not_expanded() {
        // A foreign macro call inside `Quote` is data, not a use; the expander
        // must leave it untouched (quote/quasi opacity holds for foreign macros
        // exactly as for local ones).
        let src = src_using_fixture(r#"Quote Unless(false "ran")"#);
        let (arena, roots) = expand_with_foreign(&src);
        let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
        assert!(
            printed.iter().any(|s| s.contains("unless-MACRO")),
            "quoted foreign macro call must be preserved verbatim: {printed:?}"
        );
    }
}
