use super::*;

#[test]
fn eval_expand_builtin() {
    let src = "DefMacro unless {c e} Quasi If Unquote(c) {} Unquote(e)\n\
               expand(Quote Unless(false \"ran\"))";
    assert_eq!(eval_str(src), "(if-MACRO, false, {}, \"ran\")");

    assert_eq!(eval_str("expand(Quote add(1 2))"), "(add, 1, 2)");
    assert_eq!(eval_str("expand(42)"), "42");
}

#[test]
fn eval_nested_quasi_depth() {
    assert_eq!(
        eval_str(
            "Quasi [Unquote(add(1 2)) Quasi Unquote(add(1 2)) Quasi Unquote(Unquote(add(1 2)))]"
        ),
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
    let src = "Def f Fn {path force} [path force]";
    assert_eq!(
        eval_str(&format!("{src} f({{path: \"a\" force: true}})")),
        "[\"a\", true]"
    );
    assert_eq!(eval_str(&format!("{src} f(\"a\" true)")), "[\"a\", true]");

    assert_eq!(eval_str("Def id Fn {x} x id([1 2 3])"), "[1, 2, 3]");
    assert_eq!(eval_str("head([1 2 3])"), "1");
}

#[test]
fn eval_typed_params() {
    assert_eq!(
        eval_str("Def s Fn {phrase: string} upper(phrase) s(\"hi\")"),
        "\"HI\""
    );

    let out = eval_snippet("Def s Fn {phrase: string} phrase s(42)");
    assert!(!out.ok, "type check should reject 42 for string");
}

#[test]
fn eval_let_do_match() {
    assert_eq!(eval_str("Let {x: 2 y: mul(x 3)} add(x y)"), "8");
    assert_eq!(eval_str("Do [drop(\"\") 7]"), "7");
    assert_eq!(eval_str("Match ok(5) [ (ok(n) add(n 1)) (err(e) 0) ]"), "6");
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
    assert_eq!(
        eval_str("Let {x: 2} Quasi add(1 Unquote(x))"),
        "(add, 1, 2)"
    );
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
    assert_eq!(
        eval_str("Let {c: cell-new(1)} Do [cell-set(c 5) cell-get(c)]"),
        "5"
    );
    assert_eq!(eval_str("fold(Fn {a b} add(a b) 0 range(1 5))"), "10");
    assert_eq!(eval_str("to-string({a: 1})"), "\"{a: 1}\"");
}

#[test]
fn eval_record_construct_and_match() {
    assert_eq!(
        eval_str(
            "Let {p: {x: 3 y: 7 label: \"pt\"}}\n\
                  Match p [({x: a y: b label: l} l) (other \"no\")]"
        ),
        "\"pt\""
    );
}

#[test]
fn eval_variant_and_tuple_match() {
    assert_eq!(eval_str("Match ok(42) [(ok(n) n) (err(e) 0)]"), "42");
    assert_eq!(
        eval_str("Match none [(none \"y\") (some(x) \"n\")]"),
        "\"y\""
    );

    assert_eq!(
        eval_str("Match Quote (10 20) [((a b) add(a b)) (other 0)]"),
        "30"
    );
}
