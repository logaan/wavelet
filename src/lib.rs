pub mod build;
pub mod builtins;
pub mod emit;
pub mod expand;
pub mod form;
pub mod interp;
pub mod lexer;
pub mod printer;
pub mod reader;
pub mod repl;
pub mod runner;
pub mod value;
pub mod wit;

pub use form::{Arena, Node, NodeId};
pub use lexer::ReadError;
pub use printer::print;
pub use reader::read_file;

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
        assert_eq!(read1("f(x)"), "f(x)");
        assert_eq!(read1("f(x y)"), "f((x, y))");
        assert_eq!(read1("f[x y]"), "f([x, y])");
        assert_eq!(read1("f{a: 1 b: 2}"), "f({a: 1, b: 2})");
        assert_eq!(read1("f[]"), "f([])");
        assert_eq!(read1("f()"), "f([])");
        assert_eq!(read1("kv/get{bucket: b}"), "kv/get({bucket: b})");
        assert_eq!(read1("(a b)"), "(a, b)");
        assert_eq!(read1("(a)"), "a");
        assert_eq!(read1("[a b]"), "[a, b]");
        assert_eq!(read1("{k: v}"), "{k: v}");
        assert_eq!(read1("{read write}"), "{read, write}");
        assert_eq!(read1("{}"), "{}");
        assert_eq!(read1("If c t e"), "if-MACRO((c, t, e))");
        assert_eq!(read1("Unquote(x)"), "unquote-MACRO(x)");
    }

    #[test]
    fn commas_are_whitespace() {
        assert_eq!(read1("[1, 2, 3]"), read1("[1 2 3]"));
        assert_eq!(read1("f(x, y)"), "f((x, y))");
    }

    #[test]
    fn attachment_rule() {
        assert_eq!(
            read1(r#"delete-file{path: "foo.md" force: true}"#),
            r#"delete-file({path: "foo.md", force: true})"#
        );
        assert_eq!(
            read_all(r#"delete-file {path: "foo.md" force: true}"#),
            vec!["delete-file".to_string(), r#"{path: "foo.md", force: true}"#.to_string()]
        );
    }

    #[test]
    fn macro_arity_reading() {
        assert_eq!(read1("Quote foo"), "quote-MACRO(foo)");
        assert_eq!(
            read1(r#"If eq[foo bar] print("match") print("nope")"#),
            r#"if-MACRO((eq([foo, bar]), print("match"), print("nope")))"#
        );
        // nested macro forms need no delimiters
        assert_eq!(
            read1("Def run Fn {} If c a b"),
            "def-MACRO((run, fn-MACRO(({}, if-MACRO((c, a, b))))))"
        );
        // explicit payload overrides arity reading
        assert_eq!(read1("If(c t e)"), "if-MACRO((c, t, e))");
    }

    #[test]
    fn def_macro_registers_arity() {
        let forms = read_all(
            "DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false\n\
             And lt[x 10] gt[x 0]",
        );
        assert_eq!(forms.len(), 2);
        assert_eq!(forms[1], "and-MACRO((lt([x, 10]), gt([x, 0])))");
    }

    #[test]
    fn unknown_macro_is_an_error() {
        assert!(read_file("Bogus 1 2").is_err());
    }

    #[test]
    fn title_case_does_not_collide_with_upper_words() {
        assert_eq!(read1("parse-JSON(x)"), "parse-JSON(x)");
        // TryLet is not a core form, so it needs an explicit payload here
        assert_eq!(read1("TryLet({a: b} c)"), "try-let-MACRO(({a: b}, c))");
    }

    #[test]
    fn comments_and_newlines() {
        assert_eq!(
            read1("// leading comment\nf(x) // trailing"),
            "f(x)"
        );
    }

    #[test]
    fn match_clause_shape() {
        assert_eq!(
            read1("Match r [ (ok(text) process(text)) (err(e) handle(e)) ]"),
            "match-MACRO((r, [(ok(text), process(text)), (err(e), handle(e))]))"
        );
    }

    fn eval_str(src: &str) -> String {
        let (arena, roots) = read_file(src).expect(src);
        let arena = std::rc::Rc::new(arena);
        let env = value::Env::root();
        builtins::install(&env);
        let env = env.child();
        let interp = interp::Interp::new(vec![]);
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
        assert_eq!(eval_str(src), "if-MACRO((false, {}, \"ran\"))");
        // non-macro forms pass through one step of expand unchanged
        assert_eq!(eval_str("expand(Quote add[1 2])"), "add([1, 2])");
        assert_eq!(eval_str("expand(42)"), "42");
    }

    #[test]
    fn eval_nested_quasi_depth() {
        // a nested Quasi protects its Unquotes; Unquote(Unquote(x)) fires the
        // innermost one level down
        assert_eq!(
            eval_str("Quasi [Unquote(add[1 2]) Quasi Unquote(add[1 2]) Quasi Unquote(Unquote(add[1 2]))]"),
            "[3, quasi-MACRO(unquote-MACRO(add([1, 2]))), quasi-MACRO(unquote-MACRO(3))]"
        );
    }

    #[test]
    fn eval_atoms_and_calls() {
        assert_eq!(eval_str("add[1 2]"), "3");
        assert_eq!(eval_str("str-cat[upper(\"wasm\") \"!\"]"), "\"WASM!\"");
        assert_eq!(eval_str("eq[(1 2) (1 2)]"), "true");
        assert_eq!(eval_str("If lt[1 2] \"yes\" \"no\""), "\"yes\"");
    }

    #[test]
    fn eval_def_fn_and_payload_binding() {
        // record payload binds by name, list payload by order (§4.2)
        let src = "Def f Fn {path force} (path force)";
        assert_eq!(eval_str(&format!("{src} f{{path: \"a\" force: true}}")), "(\"a\", true)");
        assert_eq!(eval_str(&format!("{src} f[\"a\" true]")), "(\"a\", true)");
        // a sole parameter receives the payload directly
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
        let interp = interp::Interp::new(vec![]);
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
        assert_eq!(eval_str("Let {x: 2 y: mul[x 3]} add[x y]"), "8");
        assert_eq!(eval_str("Do [print(\"\") 7]"), "7");
        assert_eq!(
            eval_str("Match ok(5) [ (ok(n) add[n 1]) (err(e) 0) ]"),
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
                "Def count-down Fn {n} If eq[n 0] \"liftoff\" count-down(sub[n 1])\n\
                 count-down(200000)"
            ),
            "\"liftoff\""
        );
    }

    #[test]
    fn eval_closures_capture() {
        assert_eq!(
            eval_str("Def make Fn {n} Fn {m} add[n m] Def add5 make(5) add5(3)"),
            "8"
        );
        assert_eq!(eval_str("map[Fn {x} mul[x x] [1 2 3]]"), "[1, 4, 9]");
    }

    #[test]
    fn eval_quote_quasi_macro() {
        assert_eq!(eval_str("Quote add[1 2]"), "add([1, 2])");
        assert_eq!(eval_str("Let {x: 2} Quasi add[1 Unquote(x)]"), "add([1, 2])");
        assert_eq!(eval_str("Quasi [1 Splice([2 3]) 4]"), "[1, 2, 3, 4]");
        assert_eq!(
            eval_str(
                "DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false\n\
                 And lt[1 2] lt[2 3]"
            ),
            "true"
        );
        assert_eq!(
            eval_str(
                "DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false\n\
                 And lt[2 1] boom-unbound(1)"
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
Def half Fn {n} If eq[rem[n 2] 0] ok(div[n 2]) err(\"odd\")
Def quarter Fn {n}
  TryLet {h: half(n)}
  TryLet {q: half(h)}
  ok(q)
(quarter(12) quarter(6))";
        assert_eq!(eval_str(src), "(ok(3), err(\"odd\"))");
    }

    #[test]
    fn eval_cells_and_misc() {
        assert_eq!(eval_str("Let {c: cell-new(1)} Do [cell-set[c 5] cell-get(c)]"), "5");
        assert_eq!(eval_str("fold[Fn {a b} add[a b] 0 range[1 5]]"), "10");
        assert_eq!(eval_str("to-string({a: 1})"), "\"{a: 1}\"");
        assert_eq!(eval_str("read(\"ok(5)\")"), "ok(ok(5))");
    }

    #[test]
    fn wit_synthesis_matches_spec() {
        // §6.1: the exact WIT the design doc shows for shout.wvl
        let src = "Package \"demo:shout@0.1.0\"\n\
                   Export shout\n\
                   Def shout Fn {phrase: string}\n\
                     str-cat[upper(phrase) \"!\"]";
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
                   Target \"wasi:cli/command\"\n\
                   Import {pkg: \"demo:shout/api\" as: sh}\n\
                   Export run\n\
                   Def run Fn {} println(\"hi\")";
        let (arena, roots) = read_file(src).unwrap();
        let got = wit::synthesize(&arena, &roots).unwrap();
        assert!(got.contains("run: func();"), "{got}");
        assert!(got.contains("include wasi:cli/command;"), "{got}");
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
                   Target \"wasi:cli/command\"\n\
                   Export run\n\
                   Def run Fn {}\n\
                     println(Match args() [\n\
                       ([\"wasm\"] \"WASM!\")\n\
                       ([w] str-cat[\"got \" w])\n\
                       (other \"usage\")])";
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
                   Target \"wasi:cli/command\"\n\
                   Export run\n\
                   Def make-adder Fn {n: s64}\n\
                     Fn {m: s64} add(n m)\n\
                   Def twice Fn {f x} f(f(x))\n\
                   Def inc Fn {n: s64} add(n 1)\n\
                   Def add5 make-adder(5)\n\
                   Def run Fn {}\n\
                     Do [\n\
                       println(to-string(add5(3)))\n\
                       println(to-string(twice(add5 10)))\n\
                       println(to-string(twice(inc neg(1))))]";
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
                   Target \"wasi:cli/command\"\n\
                   Def greeting str-cat[\"hello\" \", world\"]\n\
                   Export run\n\
                   Def run Fn {}\n\
                     Do [\n\
                       println(greeting)\n\
                       println(Match [\"a\" \"b\"] [\n\
                         ([\"a\" x] x)\n\
                         (other \"no\")])]";
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
                   Target \"wasi:cli/command\"\n\
                   Export run\n\
                   Def run Fn {}\n\
                     Let {p: {x: 3 y: 7 label: \"pt\"}}\n\
                       Match p [\n\
                         ({x: a label: l} println(str-cat[l to-string(a)]))\n\
                         (other println(\"no\"))]";
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
        // variant constructors (some/ok/err/none), variant patterns, tuple
        // literals + tuple patterns, all in the wasm backend
        let src = "Package \"demo:var@0.1.0\"\n\
                   Target \"wasi:cli/command\"\n\
                   Def describe Fn {r}\n\
                     Match r [\n\
                       (ok(n) to-string(n))\n\
                       (err(e) e)\n\
                       (none \"nothing\")\n\
                       (some(x) to-string(x))]\n\
                   Def pair-sum Fn {p}\n\
                     Match p [((a b) add(a b)) (other 0)]\n\
                   Export run\n\
                   Def run Fn {}\n\
                     Do [\n\
                       println(describe(ok(42)))\n\
                       println(describe(none))\n\
                       println(to-string(pair-sum((10 20))))]";
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
        assert_eq!(eval_str("Match (10 20) [((a b) add(a b)) (other 0)]"), "30");
    }

    #[test]
    fn doc_comments_attach_and_reach_wit() {
        let src = "Package \"demo:doc@0.1.0\"\n\
                   /// A pair.\n\
                   DefType point {x: s32 y: s32}\n\
                   /// Shouts.\n\
                   /// Loudly.\n\
                   Export shout\n\
                   Def shout Fn {phrase: string} upper(phrase)";
        let (arena, roots) = read_file(src).unwrap();
        assert_eq!(arena.doc(roots[1]), Some("A pair."));
        assert_eq!(arena.doc(roots[2]), Some("Shouts.\nLoudly."));
        let got = wit::synthesize(&arena, &roots).unwrap();
        assert!(got.contains("  /// A pair.\n  record point"), "{got}");
        assert!(got.contains("  /// Shouts.\n  /// Loudly.\n  shout: func"), "{got}");
    }

    #[test]
    fn wit_grouped_exports() {
        let src = "Package \"demo:gfx@0.1.0\"\n\
                   Export {iface: \"render\" name: frame}\n\
                   Def frame Fn {label: string} str-cat[\"<\" label \">\"]\n\
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
                   Def helper Fn {x: s64} mul[x x]\n\
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
            "f((x, y))",
            "if-MACRO((c, t, e))",
            r#"delete-file({path: "foo.md", force: true})"#,
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

        // main.wvl: targets wasi:cli/command, imports demo:shout/api
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
                    Target \"wasi:cli/command\"\n\
                    Import {pkg: \"demo:lists/api\" as: lst}\n\
                    Export run\n\
                    Def run Fn {}\n\
                      Do [\n\
                        println(lst/first-up{words: [\"hello\"]})\n\
                        println(head(lst/echo{words: args()}))\n\
                        println(to-string(lst/sum3{ns: [10 20 12]}))]";
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
                    Target \"wasi:cli/command\"\n\
                    Import {pkg: \"demo:geo/api\" as: g}\n\
                    Export run\n\
                    Def run Fn {}\n\
                      Let {p: g/make-point{x: 3 y: 39}}\n\
                        println(to-string(g/sum-coords{p: p}))";
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
            },
        );
        let bytes = emit::emit_component(&ma, &mr, &minfo, &deps)
            .expect("record consumer componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn aot_expansion_feeds_the_wasm_backend() {
        let src = r#"
            Package "demo:twice@0.1.0"
            DefMacro twice {x} Quasi mul[2 Unquote(x)]
            Export double
            Def double Fn {n: s64} Twice(n)
        "#;
        let (arena, roots) = read_file(src).unwrap();
        let (arena, roots) = expand::expand_file(arena, &roots).unwrap();
        // the DefMacro form is gone and the call site is rewritten
        let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
        assert!(printed.iter().all(|s| !s.contains("def-macro")));
        assert!(printed.iter().any(|s| s.contains("mul([2, n])")), "{printed:?}");
        // and the expanded tree compiles to a component
        let info = wit::collect(&arena, &roots).unwrap();
        let bytes = emit::emit_component(&arena, &roots, &info, &Default::default())
            .expect("expanded file componentizes");
        assert_eq!(&bytes[0..4], b"\0asm");
    }
}
