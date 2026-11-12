use wavelet::host::{HostComponent, Val};

fn component(source: &str) -> HostComponent {
    let dir =
        std::env::temp_dir().join(format!("wavelet-vault-regressions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("app.wlt");
    std::fs::write(&path, source).unwrap();
    let output = wavelet::build::build_files(
        &[path.to_str().unwrap().to_owned()],
        dir.join("out").to_str().unwrap(),
    )
    .unwrap();
    let bytes = std::fs::read(&output[0]).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    HostComponent::from_bytes(&bytes).unwrap()
}

#[test]
fn variant_declarations_require_one_payload_type() {
    let source = "DefType shape [rect(u32 u32) dot]\nrect(3 4)";
    let result = wavelet::eval_snippet(source);
    assert!(!result.ok);
    assert!(
        result.error.contains("at most one payload type"),
        "{}",
        result.error
    );
    let (arena, roots) = wavelet::reader::read_file("DefType shape [rect(u32 u32) dot]").unwrap();
    let wavelet::form::Node::Tup(items) = arena.node(roots[0]) else {
        panic!()
    };
    let error = wavelet::wit::type_decl(&arena, "shape", items[2]).unwrap_err();
    assert!(error.contains("tuple(...)"), "{error}");
}

#[test]
fn variants_shadow_builtins_and_unary_arithmetic_wraps() {
    let mut c = component(
        r#"Package "test:vault@0.1.0"
DefType choice [not(u32) zip(u32) tuple2(u32) other]
Export {name: shadow params: {} result: string}
Def shadow Fn {} str-cat(to-string(not(5)) str-cat(to-string(zip(30)) to-string(tuple2(7))))
Export {name: negate params: {n: s64} result: s64}
Def negate Fn {n: s64} neg(n)
Export {name: absolute params: {n: s64} result: s64}
Def absolute Fn {n: s64} abs(n)
Export {name: boxed-negate params: {n: s64} result: list(s64)}
Def boxed-negate Fn {n: s64} map(Fn {x} neg(x) [n])
"#,
    );
    let iface = "test:vault/api@0.1.0";
    assert_eq!(
        c.call_instance(iface, "shadow", &[]).unwrap(),
        vec![Val::String("not(5)zip(30)tuple2(7)".into())]
    );
    let oracle = wavelet::eval_snippet(
        "DefType choice [not(u32) zip(u32) tuple2(u32) other]\nstr-cat(to-string(not(5)) str-cat(to-string(zip(30)) to-string(tuple2(7))))",
    );
    assert!(oracle.ok, "{}", oracle.error);
    assert_eq!(oracle.value, "\"not(5)zip(30)tuple2(7)\"");
    for name in ["negate", "absolute"] {
        assert_eq!(
            c.call_instance(iface, name, &[Val::S64(-42)]).unwrap(),
            vec![Val::S64(42)]
        );
    }
    for name in ["negate", "absolute"] {
        assert_eq!(
            c.call_instance(iface, name, &[Val::S64(i64::MIN)]).unwrap(),
            vec![Val::S64(i64::MIN)]
        );
    }
    assert_eq!(
        c.call_instance(iface, "boxed-negate", &[Val::S64(i64::MIN)])
            .unwrap(),
        vec![Val::List(vec![Val::S64(i64::MIN)])]
    );
    for name in ["neg", "abs"] {
        let result = wavelet::eval_snippet(&format!("{name}(-9223372036854775808)"));
        assert!(result.ok, "{}", result.error);
        assert_eq!(result.value, "-9223372036854775808");
    }
}
