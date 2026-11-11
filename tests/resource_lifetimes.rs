const COUNTER: &str = r#"
DefResource counter {
  New: Fn {start: u32} cell-new(start)
  value: Fn {self: counter} The u32 cell-get(self)
  Drop: Fn {self: counter} {}
}
Def inspect Fn {c: borrow(counter)} The u32 counter/value(c)
Def consume Fn {c: counter} The u32 counter/value(c)
"#;

#[test]
fn borrowing_preserves_the_owner_and_teardown_drops_once() {
    let result = wavelet::eval_snippet(&format!(
        "{COUNTER}\nLet {{c: counter(7)}} Do [inspect(c) inspect(c) counter/value(c)]"
    ));
    assert!(result.ok, "{}", result.error);
    assert_eq!(result.value, "7");
}

fn oracle(source: &str) -> (wavelet::interp::Interp, wavelet::value::Env) {
    let (arena, roots) = wavelet::reader::read_file(source).unwrap();
    let arena = std::rc::Rc::new(arena);
    let interp = wavelet::interp::Interp::new();
    let env = wavelet::value::Env::root();
    wavelet::builtins::install(&env);
    for root in roots {
        interp.eval(&arena, root, &env).unwrap();
    }
    (interp, env)
}

#[test]
fn boundary_ownership_transfer_invalidates_the_callers_handle() {
    use wavelet::value::Value;
    let (interp, env) = oracle(COUNTER);
    let handle = interp
        .apply(&env.lookup("counter").unwrap(), Value::Int(7))
        .unwrap();
    assert_eq!(
        interp
            .apply(&env.lookup("consume").unwrap(), handle.clone())
            .unwrap(),
        Value::Int(7)
    );
    let error = interp
        .apply(&env.lookup("counter-value").unwrap(), handle)
        .unwrap_err();
    assert!(error.msg.contains("consumed"), "{error}");
}

#[test]
fn boundary_transfer_leaves_the_recipients_handle_usable() {
    use wavelet::value::Value;
    let (interp, env) = oracle(&format!(
        "{COUNTER}\nDef transfer Fn {{c: counter}} The counter c"
    ));
    let handle = interp
        .apply(&env.lookup("counter").unwrap(), Value::Int(9))
        .unwrap();
    let received = interp
        .apply(&env.lookup("transfer").unwrap(), handle)
        .unwrap();
    assert_eq!(
        interp
            .apply(&env.lookup("counter-value").unwrap(), received)
            .unwrap(),
        Value::Int(9)
    );
}

#[test]
fn borrowed_boundary_handle_cannot_be_moved() {
    use wavelet::value::Value;
    let (interp, env) = oracle(&format!(
        "{COUNTER}\nDef bad Fn {{c: borrow(counter)}} consume(c)"
    ));
    let handle = interp
        .apply(&env.lookup("counter").unwrap(), Value::Int(1))
        .unwrap();
    let error = interp
        .apply(&env.lookup("bad").unwrap(), handle)
        .unwrap_err();
    assert!(error.msg.contains("borrowed"), "{error}");
}

#[test]
fn local_ownership_transfer_and_callable_aliases_are_rejected() {
    for suffix in [
        "consume(counter(1))",
        "Let {c: counter(1)} Quasi Unquote(consume(c))",
        "The u32 consume(counter(1))",
        "Let {f: consume} f(counter(1))",
        "Def f consume\nf(counter(1))",
        "Let {f: Fn {c: counter} counter-value(c)} f(counter(1))",
        "map(consume [counter(1)])",
        "Def take Fn {cs: list(counter)} 0\ntake([counter(1)])",
        "Export {name: take params: {c: counter} result: u32}\nDef take Fn {c} counter-value(c)\ntake(counter(1))",
        "DefType handle counter\nDef take Fn {c: handle} 0\ntake(counter(1))",
    ] {
        let result = wavelet::eval_snippet(&format!("{COUNTER}\n{suffix}"));
        assert!(!result.ok, "{suffix}");
        assert!(
            result.error.contains("same-component ownership transfer"),
            "{suffix}: {}",
            result.error
        );
    }
}

#[test]
fn local_resource_operations_cannot_take_owned_handles() {
    for suffix in [
        "DefResource holder {New: Fn {c: counter} cell-new(0)}\nholder(counter(1))",
        "DefResource holder {New: Fn {} cell-new(0) take: Static Fn {c: counter} The u32 counter-value(c)}\nholder-take(counter(1))",
        "DefResource holder {New: Fn {} cell-new(0) take: Fn {self: holder c: counter} The u32 counter-value(c)}\nholder-take(holder() counter(1))",
    ] {
        let result = wavelet::eval_snippet(&format!("{COUNTER}\n{suffix}"));
        assert!(!result.ok, "{suffix}");
        assert!(
            result.error.contains("same-component ownership transfer"),
            "{}",
            result.error
        );
    }
}

#[test]
fn borrowed_aliases_and_shadowed_function_names_are_allowed() {
    for suffix in [
        "Let {f: inspect} f(counter(4))",
        "Def f inspect\nf(counter(4))",
        "Let {consume: Fn {n: s64} n} consume(4)",
    ] {
        let result = wavelet::eval_snippet(&format!("{COUNTER}\n{suffix}"));
        assert!(result.ok, "{suffix}: {}", result.error);
        assert_eq!(result.value, "4");
    }
}

#[test]
fn teardown_runs_every_destructor_once_even_when_one_fails() {
    use std::rc::Rc;
    use wavelet::{
        interp::Interp,
        value::{Env, Value},
    };
    let source = r#"
Def log cell-new(0)
DefResource item {
  New: Fn {n: s64} n
  Drop: Fn {self: item} Do [cell-set(log add(cell-get(log) 1)) div(1 0)]
}
Let {a: item(1) b: item(2)} 42
"#;
    let (arena, roots) = wavelet::reader::read_file(source).unwrap();
    let arena = Rc::new(arena);
    let interp = Interp::new();
    let env = Env::root();
    wavelet::builtins::install(&env);
    for root in roots {
        interp.eval(&arena, root, &env).unwrap();
    }
    assert!(interp.teardown().is_err());
    let Value::Cell(log) = env.lookup("log").unwrap() else {
        panic!()
    };
    assert_eq!(*log.borrow(), Value::Int(2));
    interp.teardown().unwrap();
    assert_eq!(*log.borrow(), Value::Int(2));
}

#[test]
fn persistent_state_rejects_shared_nested_and_cyclic_cells() {
    for source in [
        "DefResource r {New: Fn {} Let {c: cell-new(1)} tuple2(c c)}",
        "DefResource r {New: Fn {} cell-new(cell-new(1))}",
        "DefResource r {New: Fn {} Let {c: cell-new(1)} Do [cell-set(c c) c]}",
        "Def make Fn {} cell-new(1)\nDefResource r {New: Fn {} cell-new(make())}",
        "Def cell-get Fn {n: s64} cell-new(n)\nDefResource r {New: Fn {} cell-new(cell-get(1))}",
    ] {
        let result = wavelet::eval_snippet(source);
        assert!(!result.ok, "unsafe state accepted: {source}");
        assert!(
            result.error.contains("plain data") || result.error.contains("sharing or cycles"),
            "{}",
            result.error
        );
    }
}

#[test]
fn persistent_state_accepts_a_cell_of_structured_data() {
    let source = "DefResource r {New: Fn {n: s64} cell-new({count: n values: [1 2]})}\nr(4)";
    let result = wavelet::eval_snippet(source);
    assert!(result.ok, "{}", result.error);
    assert_eq!(result.value, "<r>");
}

#[test]
fn macros_can_quote_owned_function_names_without_calling_them() {
    let result = wavelet::eval_snippet(&format!(
        "{COUNTER}\nDefMacro show-name {{name}} Quasi Quote Unquote(name)\nShow-name consume"
    ));
    assert!(result.ok, "{}", result.error);
}

#[test]
fn variant_pattern_heads_do_not_hide_owned_functions() {
    let source = COUNTER.replace("Def consume Fn {c: counter} The u32 counter/value(c)", "");
    let result = wavelet::eval_snippet(&format!(
        "{source}\nDefType tag [consume(u32) other]\nDef consume Fn {{c: counter}} The u32 counter-value(c)\nLet {{v: The tag other c: counter(1)}} Match v [(consume(x) consume(c)) (other 0)]"
    ));
    assert!(!result.ok);
    assert!(
        result.error.contains("same-component ownership transfer"),
        "{}",
        result.error
    );
}

#[test]
fn owned_handles_cannot_hide_inside_named_aggregate_types() {
    for ty in [
        "{handle: counter}",
        "[held(counter) empty]",
        "list(counter)",
        "option(counter)",
        "tuple(u32 counter)",
        "result(u32 counter)",
        "result(counter string)",
    ] {
        let source = format!(
            "{COUNTER}\nDefType container {ty}\nDef take Fn {{value: container}} 0\nDef alias take"
        );
        let result = wavelet::eval_snippet(&source);
        assert!(!result.ok, "{ty}");
        assert!(
            result.error.contains("same-component ownership transfer"),
            "{ty}: {}",
            result.error
        );
    }
}

#[test]
fn ownership_guard_follows_quasiquote_depth_and_container_holes() {
    for expression in [
        "Quasi [Unquote(consume(counter(1)))]",
        "Quasi {value: Unquote(consume(counter(1)))}",
        "Quasi tuple2(1 Unquote(consume(counter(1))))",
        "Quasi Quasi Unquote Unquote(consume(counter(1)))",
    ] {
        let result = wavelet::eval_snippet(&format!("{COUNTER}\n{expression}"));
        assert!(!result.ok, "{expression}");
        assert!(
            result.error.contains("same-component ownership transfer"),
            "{expression}: {}",
            result.error
        );
    }
    let result = wavelet::eval_snippet(&format!(
        "{COUNTER}\nQuasi Quasi Unquote(consume(counter(1)))"
    ));
    assert!(result.ok, "{}", result.error);
}
