use super::*;

fn emit_test_component(source: &str) -> Result<Vec<u8>, String> {
    let (arena, roots) = read_file(source).map_err(|error| error.to_string())?;
    let (arena, roots) = crate::expand::expand_file(arena, &roots, None)?;
    let info = wit::collect(&arena, &roots)?;
    emit::emit_component(&arena, &roots, &info, &Default::default())
}

fn test_dependency(arena: &Arena, info: &wit::FileInfo) -> emit::Dep {
    emit::Dep {
        package: info.package.clone(),
        funcs: info.exports.clone(),
        package_wit: emit::dep_package_wit(arena, info).unwrap(),
        types: emit::dep_record_types(arena, info),
        type_defs: Vec::new(),
        aliases: Vec::new(),
        type_ifaces: Vec::new(),
    }
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
    let bytes = emit_test_component(src).unwrap();
    assert!(bytes.starts_with(b"\0asm"));
}

#[test]
fn emit_rejects_runtime_closure_returns() {
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
    let error = emit_test_component(src).unwrap_err();
    assert!(
        error.contains("function types cannot cross component boundaries"),
        "{error}"
    );
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
    let bytes = emit_test_component(src).unwrap();
    assert!(bytes.starts_with(b"\0asm"));
}

#[test]
fn emit_records_componentize() {
    let src = "Package \"demo:rec@0.1.0\"\n\
               Export run\n\
               Def run Fn {}\n\
                 Let {p: {x: 3 y: 7 label: \"pt\"}}\n\
                   Match p [\n\
                     ({x: a label: l} str-cat(l to-string(a)))\n\
                     (other \"no\")]";
    let bytes = emit_test_component(src).unwrap();
    assert!(bytes.starts_with(b"\0asm"));
}

#[test]
fn emit_single_curated_overload_export_componentize() {
    let src = "Package \"demo:eqs@0.1.0\"\n\
               DefType point {x: s32 y: s32}\n\
               Def eq Fn {a: point b: point} true\n\
               Export eq";
    let bytes = emit_test_component(src).unwrap();
    assert!(bytes.starts_with(b"\0asm"));
}

#[test]
fn emit_two_member_overload_export_componentize() {
    let src = "Package \"demo:eqo@0.1.0\"\n\
               DefType point {x: s32 y: s32}\n\
               Def eq Fn {a: point b: point} true\n\
               Def eq Fn {a: string b: string} true\n\
               Export eq";
    let (arena, roots) = read_file(src).unwrap();
    let (arena, roots) = crate::expand::expand_file(arena, &roots, None).unwrap();
    let info = wit::collect(&arena, &roots).unwrap();

    assert!(info.exports.iter().any(|s| s.name == "eq-point"));
    assert!(info.exports.iter().any(|s| s.name == "eq-string"));
    let bytes =
        emit::emit_component(&arena, &roots, &info, &std::collections::HashMap::new()).unwrap();
    assert!(bytes.starts_with(b"\0asm"));
}

#[test]
fn emit_rejects_untyped_variant_mixing() {
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
    let error = emit_test_component(src).unwrap_err();
    assert!(error.contains("cannot infer a static type"), "{error}");
}

#[test]
fn emit_statically_bound_closures_and_typed_compounds() {
    let source = r#"
Package "demo:typed@0.1.0"
Def describe Fn {r: result(s64 string)}
  Match r [(ok(n) to-string(n)) (err(e) e)]
Export {name: run result: string}
Def run Fn {}
  Let {n: 5}
    Let {add-five: Fn {m: s64} add(n m)}
      Let {pair: some(tuple2(add-five(3) add-five(4)))}
        Match pair [(some((a b)) describe(ok(add(a b)))) (none "nothing")]
"#;
    let bytes = emit_test_component(source).unwrap();
    let mut host = crate::host::HostComponent::from_bytes(&bytes).unwrap();
    assert_eq!(
        host.call_instance("demo:typed/api@0.1.0", "run", &[])
            .unwrap(),
        vec![crate::host::Val::String("17".to_owned())]
    );
}

#[test]
fn emit_components_for_spec_demo() {
    let (sa, sr) = read_file(include_str!("../../examples/shout.wlt")).unwrap();
    let (sa, sr) = crate::expand::expand_file(sa, &sr, None).unwrap();
    let sinfo = wit::collect(&sa, &sr).unwrap();
    let bytes =
        emit::emit_component(&sa, &sr, &sinfo, &Default::default()).expect("shout componentizes");
    assert_eq!(&bytes[0..4], b"\0asm");

    let (ma, mr) = read_file(include_str!("../../examples/main.wlt")).unwrap();
    let (ma, mr) = crate::expand::expand_file(ma, &mr, None).unwrap();
    let minfo = wit::collect(&ma, &mr).unwrap();
    let mut deps = std::collections::HashMap::new();
    deps.insert("demo:shout".to_string(), test_dependency(&sa, &sinfo));
    let bytes = emit::emit_component(&ma, &mr, &minfo, &deps).expect("main componentizes");
    assert_eq!(&bytes[0..4], b"\0asm");
}

#[test]
fn emit_lists_across_boundaries() {
    let psrc = "Package \"demo:lists@0.1.0\"\n\
                Export {name: first-up params: {words: list(string)} result: string}\n\
                Def first-up Fn {words}\n\
                  If gt(len(words) 0) str-cat(head(words)) \"EMPTY\"\n\
                Export {name: echo params: {words: list(string)} result: list(string)}\n\
                Def echo Fn {words} words\n\
                Export {name: sum3 params: {ns: list(s64)} result: s64}\n\
                Def sum3 Fn {ns}\n\
                  Match ns [([a b c] add(a add(b c))) (other 0)]";
    let (pa, pr) = read_file(psrc).unwrap();
    let (pa, pr) = crate::expand::expand_file(pa, &pr, None).unwrap();
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
    let (ma, mr) = crate::expand::expand_file(ma, &mr, None).unwrap();
    let minfo = wit::collect(&ma, &mr).unwrap();
    let mut deps = std::collections::HashMap::new();
    deps.insert("demo:lists".to_string(), test_dependency(&pa, &pinfo));
    let bytes = emit::emit_component(&ma, &mr, &minfo, &deps).expect("list consumer componentizes");
    assert_eq!(&bytes[0..4], b"\0asm");
}

#[test]
fn emit_records_across_boundaries() {
    let psrc = "Package \"demo:geo@0.1.0\"\n\
                DefType point {x: s64 y: s64}\n\
                Export {name: make-point params: {x: s64 y: s64} result: point}\n\
                Def make-point Fn {x: s64 y: s64} {x: x y: y}\n\
                Export {name: sum-coords params: {p: point} result: s64}\n\
                Def sum-coords Fn {p}\n\
                  Match p [({x: a y: b} add(a b)) (other 0)]";
    let (pa, pr) = read_file(psrc).unwrap();
    let (pa, pr) = crate::expand::expand_file(pa, &pr, None).unwrap();
    let pinfo = wit::collect(&pa, &pr).unwrap();

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
    let (ma, mr) = crate::expand::expand_file(ma, &mr, None).unwrap();
    let minfo = wit::collect(&ma, &mr).unwrap();
    let mut deps = std::collections::HashMap::new();
    deps.insert("demo:geo".to_string(), test_dependency(&pa, &pinfo));
    let bytes =
        emit::emit_component(&ma, &mr, &minfo, &deps).expect("record consumer componentizes");
    assert_eq!(&bytes[0..4], b"\0asm");
}

#[test]
fn emit_option_result_across_boundaries() {
    let psrc = "Package \"demo:opt@0.1.0\"\n\
                Export {name: lookup params: {k: s64} result: option(s64)}\n\
                Def lookup Fn {k} If gt(k 0) some(mul(k 10)) none\n\
                Export {name: checked params: {k: s64} result: result(s64 string)}\n\
                Def checked Fn {k} If gt(k 0) ok(k) err(\"nonpositive\")";
    let (pa, pr) = read_file(psrc).unwrap();
    let (pa, pr) = crate::expand::expand_file(pa, &pr, None).unwrap();
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
    let (ma, mr) = crate::expand::expand_file(ma, &mr, None).unwrap();
    let minfo = wit::collect(&ma, &mr).unwrap();
    let mut deps = std::collections::HashMap::new();
    deps.insert("demo:opt".to_string(), test_dependency(&pa, &pinfo));
    let bytes = emit::emit_component(&ma, &mr, &minfo, &deps)
        .expect("option/result consumer componentizes");
    assert_eq!(&bytes[0..4], b"\0asm");
}

#[test]
fn emit_list_of_records_across_boundaries() {
    let psrc = "Package \"demo:lr2@0.1.0\"\n\
                DefType pt {x: s64 y: s64}\n\
                Export {name: pts params: {n: s64} result: list(pt)}\n\
                Def pts Fn {n} [{x: n y: mul(n 2)} {x: add(n 1) y: 0}]";
    let (pa, pr) = read_file(psrc).unwrap();
    let (pa, pr) = crate::expand::expand_file(pa, &pr, None).unwrap();
    let pinfo = wit::collect(&pa, &pr).unwrap();
    let bytes = emit::emit_component(&pa, &pr, &pinfo, &Default::default())
        .expect("list<record> provider componentizes");
    assert_eq!(&bytes[0..4], b"\0asm");
}

#[test]
fn emit_list_fields_in_aggregates_across_boundaries() {
    let psrc = "Package \"demo:lrec@0.1.0\"\n\
                DefType bag {tag: string items: list(s64)}\n\
                Export {name: mk params: {tag: string items: list(s64)} result: bag}\n\
                Def mk Fn {tag: string items: list(s64)} {tag: tag items: items}\n\
                Export {name: maybe params: {k: s64} result: option(list(s64))}\n\
                Def maybe Fn {k} If gt(k 0) some([k mul(k 2)]) none";
    let (pa, pr) = read_file(psrc).unwrap();
    let (pa, pr) = crate::expand::expand_file(pa, &pr, None).unwrap();
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

    let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
    assert!(printed.iter().all(|s| !s.contains("def-macro")));
    assert!(
        printed.iter().any(|s| s.contains("(mul, 2, n)")),
        "{printed:?}"
    );

    let info = wit::collect(&arena, &roots).unwrap();
    let bytes = emit::emit_component(&arena, &roots, &info, &Default::default())
        .expect("expanded file componentizes");
    assert_eq!(&bytes[0..4], b"\0asm");
}
