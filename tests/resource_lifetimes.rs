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

#[test]
fn ownership_transfer_invalidates_the_callers_handle() {
    let result = wavelet::eval_snippet(&format!(
        "{COUNTER}\nLet {{c: counter(7)}} Do [consume(c) counter/value(c)]"
    ));
    assert!(!result.ok);
    assert!(result.error.contains("consumed"), "{}", result.error);
}

#[test]
fn transferred_ownership_remains_usable_by_the_recipient() {
    let result = wavelet::eval_snippet(&format!(
        "{COUNTER}\nDef transfer Fn {{c: counter}} The counter c\nLet {{c: transfer(counter(9))}} counter/value(c)"
    ));
    assert!(result.ok, "{}", result.error);
    assert_eq!(result.value, "9");
}

#[test]
fn borrowed_handle_cannot_be_moved() {
    let result = wavelet::eval_snippet(&format!(
        "{COUNTER}\nDef bad Fn {{c: borrow(counter)}} consume(c)\nbad(counter(1))"
    ));
    assert!(!result.ok);
    assert!(result.error.contains("borrowed"), "{}", result.error);
}

#[test]
fn teardown_runs_every_destructor_once_even_when_one_fails() {
    use std::rc::Rc;
    use wavelet::{interp::Interp, value::{Env, Value}};
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
    let Value::Cell(log) = env.lookup("log").unwrap() else { panic!() };
    assert_eq!(*log.borrow(), Value::Int(2));
    interp.teardown().unwrap();
    assert_eq!(*log.borrow(), Value::Int(2));
}
