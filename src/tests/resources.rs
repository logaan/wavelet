use super::*;

const COUNTER: &str = "DefResource counter {\n\
    New: Fn {start: s64} cell-new(start)\n\
    next: Fn {self: counter} Do [cell-set(self add(cell-get(self) 1)) cell-get(self)]\n\
    value: Fn {self: counter} cell-get(self)\n\
    sum: Static Fn {values: list(s64)} counter(fold(add 0 values))\n\
  }\n";

#[test]
fn eval_defresource_constructor_and_methods() {
    assert_eq!(
        eval_str(&format!(
            "{COUNTER}Def c counter(5)\n                 Do [counter/next(c) counter/next(c) counter/value(c)]"
        )),
        "7"
    );
}

#[test]
fn eval_defresource_static_alt_constructor() {
    assert_eq!(
        eval_str(&format!("{COUNTER}counter/value(counter/sum([1 2 3 4]))")),
        "10"
    );
}

#[test]
fn eval_defresource_identity_and_opaque_to_string() {
    assert_eq!(
        eval_str(&format!("{COUNTER}Def c counter(1)\neq(c c)")),
        "true"
    );
    assert_eq!(
        eval_str(&format!("{COUNTER}eq(counter(1) counter(1))")),
        "false"
    );

    assert_eq!(
        eval_str(&format!("{COUNTER}to-string(counter(9))")),
        "\"<counter>\""
    );
}

fn check_src(src: &str) -> Result<(), String> {
    let (arena, roots) = read_file(src).map_err(|e| e.msg.clone())?;
    let (xarena, xroots) = crate::expand::expand_file(arena, &roots, None)?;
    crate::check::check_program(&xarena, &xroots)
}

#[test]
fn check_defresource_valid_counter_ok() {
    assert!(
        check_src(COUNTER).is_ok(),
        "the counter resource should check"
    );
}

#[test]
fn check_defresource_static_with_self_rejected() {
    let src = "DefResource counter {\n            New: Fn {start: s64} cell-new(start)\n            bad: Static Fn {self: counter} cell-get(self)\n          }";
    let err = check_src(src).expect_err("a static member with self must be rejected");
    assert!(err.contains("must not take `self`"), "{err}");
}

#[test]
fn check_defresource_method_without_self_rejected() {
    let src = "DefResource counter {\n            New: Fn {start: s64} cell-new(start)\n            bad: Fn {n: s64} n\n          }";
    let err = check_src(src).expect_err("a method lacking self must be rejected");
    assert!(err.contains("must take"), "{err}");
    assert!(err.contains("self: counter"), "{err}");
}

#[test]
fn check_defresource_static_on_new_rejected() {
    let src = "DefResource counter {\n            New: Static Fn {start: s64} cell-new(start)\n          }";
    let err = check_src(src).expect_err("Static on New must be rejected");
    assert!(err.contains("`New` cannot be `Static`"), "{err}");
}

#[test]
fn check_defresource_static_on_drop_rejected() {
    let src = "DefResource counter {\n            New: Fn {start: s64} cell-new(start)\n            Drop: Static Fn {self: counter} unit()\n          }";
    let err = check_src(src).expect_err("Static on Drop must be rejected");
    assert!(err.contains("`Drop` cannot be `Static`"), "{err}");
}

#[test]
fn check_derive_over_resource_rejected() {
    let src = "DefResource counter {\n            New: Fn {start: s64} cell-new(start)\n          }\n          Derive {Eq} counter";
    let err = check_src(src).expect_err("Derive over a resource must be rejected");
    assert!(err.contains("cannot be used on the resource type"), "{err}");
}

#[test]
fn check_borrow_in_result_position_rejected() {
    let src = "Package \"demo:r@0.1.0\"\n          DefResource counter {\n            New: Fn {start: s64} cell-new(start)\n          }\n          Export {name: mk params: {} result: borrow(counter)}\n          Def mk Fn {} counter(0)";
    let err = check_src(src).expect_err("borrow in result position must be rejected");
    assert!(err.contains("result position"), "{err}");
}

#[test]
fn check_borrow_in_param_position_ok() {
    let src = "DefResource counter {\n            New: Fn {start: s64} cell-new(start)\n            value: Fn {self: counter} cell-get(self)\n          }\n          Def bump Fn {c: borrow(counter)} counter/value(c)";
    assert!(check_src(src).is_ok(), "borrow in a parameter is allowed");
}
