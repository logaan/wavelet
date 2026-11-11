use wavelet::host::{HostComponent, Val};

#[test]
fn oracle_constructs_tuples() {
    for (src, want) in [
        ("tuple0()", "()"),
        ("tuple1(5)", "(5)"),
        ("tuple1(tuple2(1 2))", "((1, 2))"),
        ("tuple2(1 \"a\")", "(1, \"a\")"),
        (
            "tuple3(add(1 2) \"x\" tuple2(true tuple1(5)))",
            "(3, \"x\", (true, (5)))",
        ),
        ("Match tuple2(3 4) [((a b) add(a b)) (other 0)]", "7"),
        ("eq(tuple2(1 2) Quote (1 2))", "true"),
        ("tuple2(7 8)", "(7, 8)"),
        ("map(tuple1 [1 2 3])", "[(1), (2), (3)]"),
        (
            "DefType t [tuple2(u32) other]\nMatch tuple2(30) [((tuple2 n) n) (other 0)]",
            "30",
        ),
    ] {
        let r = wavelet::eval_snippet(src);
        assert!(r.ok, "{src}: {}", r.error);
        assert_eq!(r.value, want, "{src}");
    }
}

#[test]
fn oracle_rejects_wrong_arity() {
    for src in ["tuple2(1)", "tuple0(1)", "tuple1(1 2)", "tuple3(1 2)"] {
        let r = wavelet::eval_snippet(src);
        assert!(!r.ok, "{src} should be an arity error");
        assert!(
            r.error.contains("argument"),
            "{src}: unexpected error {}",
            r.error
        );
    }
}

fn built() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-tupctor-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:tupctor@0.1.0"

Export {name: mk params: {a: u32 b: string} result: tuple(u32 string)}
Def mk Fn {a: u32 b: string} tuple2(add(a 1) str-cat(b "!"))

Export {name: one params: {s: string} result: tuple(string)}
Def one Fn {s: string} tuple1(str-cat(s "?"))

Export {name: nest params: {n: u32} result: tuple(u32 tuple(bool string))}
Def nest Fn {n: u32} tuple2(add(n 1) tuple2(true "y"))

Export {name: three params: {n: u8 s: string b: bool} result: tuple(u8 string bool)}
Def three Fn {n: u8 s: string b: bool}
  tuple3(to-u8(add(n 1)) str-cat(s ".") not(b))

Export {name: rt params: {n: u32} result: result(tuple(u32 string) string)}
Def rt Fn {n: u32} The result(tuple(u32 string) string) ok(tuple2(add(n 2) "p"))

Export {name: boxed params: {} result: string}
Def boxed Fn {}
  to-string(tuple3(1 tuple0() tuple1(2)))

Export {name: destruct params: {} result: s64}
Def destruct Fn {}
  Match tuple2(20 22) [((a b) add(a b)) (other 0)]

Export {name: shadowed params: {n: s32} result: s64}
Def shadowed Fn {n: s32}
  Let {tuple2: Fn {x: s32} add(x 1)} tuple2(n)

Export {name: rebuild params: {p: tuple(u8 u8)} result: tuple(u8 u8)}
Def rebuild Fn {p: tuple(u8 u8)} The tuple(u8 u8)
  Match p [((a b) tuple2(to-u8(add(a 1)) to-u8(add(b 1))))]
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the tuple-constructor app");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate")
}

const IFACE: &str = "demo:tupctor/api@0.1.0";

fn call(c: &mut HostComponent, f: &str, args: &[Val]) -> Val {
    c.call_instance(IFACE, f, args)
        .unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0]
        .clone()
}

#[test]
fn constructed_tuples_cross_the_boundary() {
    let mut c = built();
    assert_eq!(
        call(&mut c, "mk", &[Val::U32(4), Val::String("hi".into())]),
        Val::Tuple(vec![Val::U32(5), Val::String("hi!".into())])
    );
    assert_eq!(
        call(&mut c, "one", &[Val::String("a".into())]),
        Val::Tuple(vec![Val::String("a?".into())])
    );
    assert_eq!(
        call(&mut c, "nest", &[Val::U32(9)]),
        Val::Tuple(vec![
            Val::U32(10),
            Val::Tuple(vec![Val::Bool(true), Val::String("y".into())]),
        ])
    );
    assert_eq!(
        call(
            &mut c,
            "three",
            &[Val::U8(7), Val::String("q".into()), Val::Bool(false)]
        ),
        Val::Tuple(vec![Val::U8(8), Val::String("q.".into()), Val::Bool(true)])
    );
    let Val::Result(Ok(Some(inner))) = call(&mut c, "rt", &[Val::U32(1)]) else {
        panic!("rt should return ok(tuple)");
    };
    assert_eq!(
        *inner,
        Val::Tuple(vec![Val::U32(3), Val::String("p".into())])
    );

    assert_eq!(
        call(
            &mut c,
            "rebuild",
            &[Val::Tuple(vec![Val::U8(7), Val::U8(9)])]
        ),
        Val::Tuple(vec![Val::U8(8), Val::U8(10)])
    );
}

#[test]
fn boxed_path_agrees_with_the_oracle() {
    let mut c = built();
    assert_eq!(
        call(&mut c, "boxed", &[]),
        Val::String("(1, (), (2))".into())
    );
    assert_eq!(call(&mut c, "destruct", &[]), Val::S64(42));
    assert_eq!(call(&mut c, "shadowed", &[Val::S32(5)]), Val::S64(6));
}
