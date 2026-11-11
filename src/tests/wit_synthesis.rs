use super::*;

#[test]
fn wit_synthesis_matches_spec() {
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
fn wit_grouped_exports() {
    let src = "Package \"demo:gfx@0.1.0\"\n\
               Export {iface: \"render\" name: frame}\n\
               Def frame Fn {label: string} str-cat(\"<\" label \">\")\n\
               Export ping\n\
               Def ping Fn {} \"pong\"";
    let (arena, roots) = read_file(src).unwrap();
    let got = wit::synthesize(&arena, &roots).unwrap();
    assert!(
        got.contains("interface render {\n  frame: func(label: string) -> string;"),
        "{got}"
    );
    assert!(
        got.contains("interface api {\n  ping: func() -> string;"),
        "{got}"
    );
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
    assert!(
        got.contains("greet: func(name: string) -> string;"),
        "{got}"
    );
}
