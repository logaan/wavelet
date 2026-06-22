//! TDD plan for the monomorphic type system (`dev-notes/dd-type-system.typ`).
//!
//! Every test here describes behaviour the type system *will* have but does not
//! yet. They are all `#[ignore]`d so the normal suite stays green; each carries a
//! `// Step N` comment pointing at the implementation step (tracked in
//! `dev-notes/type-system-todo.typ`) that should turn it from red to green. As a
//! step lands, delete that step's `#[ignore]` lines and make the tests pass.
//!
//! These tests are written against today's *public* API so the crate keeps
//! compiling (a non-compiling test crate would break `cargo test` for everyone,
//! defeating the point of `#[ignore]`). Two observation channels are used:
//!
//!   * [`run`]  — the interpreter / playground path (`eval_snippet`). Used for
//!     semantics that are visible by running core forms, `Def`s and builtins.
//!     `eval_snippet` rejects file-level forms (`Package`/`Import`/`Export`/
//!     `DefType`), so whole-file behaviour is observed through `synth`.
//!   * [`synth`] — read → expand → synthesize the component's WIT world. Used for
//!     boundary behaviour: inferred signatures, overload name-mangling, functor
//!     instantiation.
//!
//! Run *only* the pending suite to confirm it is all red (the TDD baseline):
//!
//! ```console
//! cargo test --test type_system -- --ignored
//! ```
//!
//! Normal `cargo test` skips every test in this file.

use wavelet::{eval_snippet, expand, print, read_file, wit, EvalOutcome};

/// Interpreter path: read every form and evaluate in order, like the docs
/// playground and `wavelet run`. The type checker is expected to run here too,
/// so an ill-typed program reports `ok == false` even if the bad code is never
/// reached at runtime.
fn run(src: &str) -> EvalOutcome {
    eval_snippet(src)
}

/// Boundary path: read → expand → synthesize the file's WIT world. Returns the
/// WIT text, or the first compile error.
fn synth(src: &str) -> Result<String, String> {
    let (arena, roots) = read_file(src).map_err(|e| e.to_string())?;
    let (arena, roots) = expand::expand_file(arena, &roots, None)?;
    wit::synthesize(&arena, &roots)
}

/// Macro-expansion path: read → expand, then print every resulting top-level
/// form. Used to assert what a deriver/functor macro *emits*.
fn expand_forms(src: &str) -> Result<Vec<String>, String> {
    let (arena, roots) = read_file(src).map_err(|e| e.to_string())?;
    let (arena, roots) = expand::expand_file(arena, &roots, None)?;
    Ok(roots.iter().map(|&r| print(&arena, r)).collect())
}

// ===========================================================================
// Phase A — the core checker: the two rules, total monomorphic checking
// ===========================================================================

// --- Step 1: checker skeleton, literal types, total checking wired in --------

#[test]
// Step 1 — the total checker is wired into the compile path: an ill-typed
// function body is a compile error even though `bad` is never called (today the
// body never runs, so the program is wrongly accepted). This is the smoke test
// that a static checker runs at all.
fn illtyped_uncalled_def_is_a_compile_error() {
    let r = run(r#"Def bad Fn {} add("a" "b")"#);
    assert!(!r.ok, "expected a compile-time type error, got value {:?}", r.value);
}

#[test]
// Step 1 — the checker knows builtins' operand types: `add` of an `s32` and a
// `string` has no WIT type, so it is rejected (uncalled today ⇒ accepted).
fn mixed_arithmetic_operands_are_rejected() {
    let r = run(r#"Def bad Fn {x: s32} add(x "y")"#);
    assert!(!r.ok, "mixed s32 + string add should not type-check");
}

// --- Step 2: bidirectional checking of the core forms ------------------------

#[test]
// Step 2 — every expression has exactly one WIT type: an `If` whose branches are
// `s64` and `string` cannot be typed, so it is a compile error (dead branch
// today ⇒ accepted).
fn if_branches_must_share_one_wit_type() {
    let r = run(r#"Def f Fn {b: bool} If b 1 "s""#);
    assert!(!r.ok, "heterogeneous If branches must be rejected");
}

#[test]
// Step 2 — `Match` clause results must unify to one WIT type, exactly like `If`.
fn match_clause_results_must_share_one_wit_type() {
    let r = run(r#"Def f Fn {b: bool} Match b [(true 1) (false "s")]"#);
    assert!(!r.ok, "heterogeneous Match clause results must be rejected");
}

#[test]
// Step 2 — checking is total and resolves names statically: an unbound name in
// an uncalled function body is a compile error (today the body is never
// evaluated, so it slips through).
fn unbound_name_in_uncalled_body_is_a_compile_error() {
    let r = run(r#"Def f Fn {} nope"#);
    assert!(!r.ok, "unbound `nope` should be caught statically");
}

#[test]
// Step 2 — `Let` binding types flow into the body: `n` is inferred `s64`, so
// using it where a `string` is required is a compile error.
fn let_binding_types_flow_into_the_body() {
    let r = run(r#"Def f Fn {} Let {n: 1} str-cat(n "x")"#);
    assert!(!r.ok, "s64 binding used as string should be rejected");
}

// --- Step 3: `The` ascription + literal context-resolution & range checks -----

#[test]
// Step 3 — numeric literals are context-resolved with a compile-time range
// check: `300` does not fit `u8`, so `The u8 300` is a static error (uncalled
// today ⇒ no runtime check fires).
fn out_of_range_literal_for_u8_is_a_compile_error() {
    let r = run(r#"Def f Fn {} The u8 300"#);
    assert!(!r.ok, "300 is out of range for u8 and must be rejected");
}

#[test]
// Step 3 — a negative literal cannot resolve to an unsigned type.
fn negative_literal_for_unsigned_is_a_compile_error() {
    let r = run(r#"Def f Fn {} The u8 -1"#);
    assert!(!r.ok, "-1 cannot be a u8");
}

#[test]
// Step 3 — a float literal cannot resolve to an integer type.
fn float_literal_where_int_expected_is_a_compile_error() {
    let r = run(r#"Def f Fn {} The s32 1.5"#);
    assert!(!r.ok, "1.5 cannot be an s32");
}

// --- Step 4: function signatures are WIT function types; calls are checked ----

#[test]
// Step 4 — call arguments are checked against the callee's WIT signature: a
// `string` argument where the parameter is `s32` is a compile error.
fn call_argument_type_mismatch_is_a_compile_error() {
    let r = run(r#"Def g Fn {x: s32} x
Def f Fn {} g("str")"#);
    assert!(!r.ok, "string argument to an s32 parameter must be rejected");
}

#[test]
// Step 4 — call arity is checked against the signature: two arguments to a
// one-parameter function is a compile error.
fn call_arity_mismatch_is_a_compile_error() {
    let r = run(r#"Def g Fn {x: s32} x
Def f Fn {} g(1 2)"#);
    assert!(!r.ok, "arity mismatch must be rejected");
}

#[test]
// Step 4 — a typed parameter pins the type inside the body: using an `s32`
// parameter where a `string` is required is a compile error.
fn typed_parameter_used_at_wrong_type_is_rejected() {
    let r = run(r#"Def f Fn {x: s32} str-cat(x "!")"#);
    assert!(!r.ok, "s32 parameter used as string must be rejected");
}

// ===========================================================================
// Phase B — boundary synthesis from inference
// ===========================================================================

// --- Step 5: WIT synthesis driven by full inference --------------------------

#[test]
// Step 5 — an *un-annotated* export parameter gets its WIT type inferred from
// use (`upper`/`str-cat` force `string`), so the synthesized signature is
// concrete. Today untyped parameters make synthesis fail outright.
fn synthesis_infers_untyped_parameter_from_use() {
    let wit = synth(
        r#"Package "demo:shout@0.1.0"
Export shout
Def shout Fn {phrase} str-cat(upper(phrase) "!")"#,
    )
    .expect("a fully-inferable export should synthesize");
    assert!(wit.contains("phrase: string"), "param type not inferred:\n{wit}");
    assert!(wit.contains("-> string"), "result type not inferred:\n{wit}");
}

#[test]
// Step 5 — the checker knows the monomorphic shape of sequence builtins, so a
// `list<s32>`-returning body synthesizes a concrete result type (today `reverse`
// is unknown to inference and synthesis fails).
fn synthesis_infers_list_result_type() {
    let wit = synth(
        r#"Package "demo:r@0.1.0"
Export rev
Def rev Fn {xs: list(s32)} reverse(xs)"#,
    )
    .expect("list result should be inferable");
    assert!(wit.contains("-> list<s32>"), "list result not inferred:\n{wit}");
}

// ===========================================================================
// Phase C — overloading (core: needs static types, so it cannot be a library)
// ===========================================================================

// --- Step 6: overload sets + argument-directed resolution --------------------

#[test]
// Step 6 — same-named monomorphic defs form one overload set; a call resolves to
// the member matching its static argument type. Today the second `Def` shadows
// the first, so `show(5)` hits the `string` body and fails.
fn overload_resolves_by_argument_type_first_definition() {
    let r = run(r#"Def show Fn {x: s32} "int"
Def show Fn {x: string} "str"
show(5)"#);
    assert!(r.ok, "overloaded call failed: {}", r.error);
    assert_eq!(r.value, r#""int""#, "resolved to the wrong overload");
}

#[test]
// Step 6 — resolution works regardless of definition order; the shadowed-first
// member must still be reachable.
fn overload_resolves_by_argument_type_second_definition() {
    let r = run(r#"Def show Fn {x: string} "str"
Def show Fn {x: s32} "int"
show("hi")"#);
    assert!(r.ok, "overloaded call failed: {}", r.error);
    assert_eq!(r.value, r#""str""#, "resolved to the wrong overload");
}

#[test]
// Step 6 — an argument that fits two overloads equally is ambiguous: a compile
// error at the call site (fixable by qualifying). `0` fits both `u8` and `s32`.
fn ambiguous_overloaded_call_is_a_compile_error() {
    let r = run(r#"Def f Fn {x: u8} "a"
Def f Fn {x: s32} "b"
f(0)"#);
    assert!(!r.ok, "an ambiguous overloaded call must be rejected");
}

// --- Step 7: return-type-directed resolution via `The` / context -------------

#[test]
// Step 7 — when arguments don't disambiguate (here two zero-arg `make`s), the
// expected type from a `The` ascription selects the overload.
fn return_type_directed_resolution_via_the() {
    let r = run(r#"Def make Fn {} 1
Def make Fn {} "x"
The s64 make()"#);
    assert!(r.ok, "return-type-directed call failed: {}", r.error);
    assert_eq!(r.value, "1", "ascription should select the s64 overload");
}

#[test]
// Step 7 — with neither arguments nor an expected type to decide, the call is
// ambiguous and must be a compile error.
fn return_type_overload_without_context_is_a_compile_error() {
    let r = run(r#"Def make Fn {} 1
Def make Fn {} "x"
make()"#);
    assert!(!r.ok, "return-type overload with no context must be rejected");
}

// --- Step 8: name-mangling at the boundary for exported overload sets ---------

#[test]
// Step 8 — WIT has no overloading, so an exported overload set lowers to
// distinctly-named functions. Exporting `eq` for `point` yields `eq-point`.
fn exported_overload_set_is_name_mangled() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Def eq Fn {a: point b: point} true
Def eq Fn {a: string b: string} true
Export eq"#,
    )
    .expect("exported overload set should synthesize");
    assert!(wit.contains("eq-point"), "missing mangled name eq-point:\n{wit}");
    assert!(wit.contains("eq-string"), "missing mangled name eq-string:\n{wit}");
}

#[test]
// Step 8 — the mangled, monomorphic signatures are concrete WIT functions (no
// generic parameter survives to the boundary).
fn mangled_overload_signature_is_concrete() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Def eq Fn {a: point b: point} true
Export eq"#,
    )
    .expect("exported overload should synthesize");
    assert!(
        wit.contains("eq-point: func(a: point, b: point) -> bool"),
        "mangled signature is not the expected concrete WIT:\n{wit}"
    );
}

// ===========================================================================
// Phase D — the standard-library affordances, built on the core substrate
// ===========================================================================

// --- Step 9: `Derive` and the derivers Eq / Ord / Show / Hash ----------------

#[test]
// Step 9 — `Derive` is a stdlib macro (tree → tree): `Derive {Eq} point` emits a
// concrete `eq-point` definition. Asserted at expansion time.
fn derive_eq_emits_a_monomorphic_definition() {
    let forms = expand_forms(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq} point"#,
    )
    .expect("Derive should be a known macro that expands");
    assert!(
        forms.iter().any(|f| f.contains("eq-point")),
        "Derive {{Eq}} did not emit eq-point; expansion was:\n{}",
        forms.join("\n")
    );
}

#[test]
// Step 9 — a derived operation joins the overload set, so `eq(a b)` on `point`s
// resolves to the derived `eq-point` and the export synthesizes (this is the
// `fig-derive` example end-to-end).
fn derived_eq_is_resolvable_and_synthesizes() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq} point
Export same
Def same Fn {a: point b: point} eq(a b)"#,
    )
    .expect("derived eq should resolve and synthesize");
    assert!(
        wit.contains("same: func(a: point, b: point) -> bool"),
        "derived eq did not resolve at the call site:\n{wit}"
    );
}

// --- Step 10: source functors via parameterized `Import` ---------------------

#[test]
// Step 10 — a functor is a component instantiated at a concrete element type at
// compile time. `Import { … elem: point … }` stamps out a monomorphic `Set` and
// synthesizes a concrete `point-set` interface.
fn functor_instantiation_synthesizes_concrete_interface() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Import {pkg: "wavelet:coll/set" elem: point as: pts}
Export has
Def has Fn {p: point} pts/contains(pts/new() p)"#,
    )
    .expect("a functor instantiation should synthesize");
    assert!(wit.contains("point-set"), "no specialized point-set interface:\n{wit}");
}

#[test]
// Step 10 — two instantiations at different element types produce two distinct
// concrete interfaces; nothing generic is shared.
fn two_functor_instantiations_make_two_interfaces() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Import {pkg: "wavelet:coll/set" elem: point as: pts}
Import {pkg: "wavelet:coll/set" elem: string as: strs}
Export demo
Def demo Fn {p: point s: string} pts/contains(pts/new() p)"#,
    )
    .expect("two functor instantiations should synthesize");
    assert!(wit.contains("point-set"), "missing point-set:\n{wit}");
    assert!(wit.contains("string-set"), "missing string-set:\n{wit}");
}

// --- Step 11: binary-functor specialization (the one new core functor pass) --

#[test]
// Step 11 — a precompiled (binary) parameterized component is monomorphized by
// substituting the element type into its WIT. The specialized resource's own
// methods (e.g. `size`) appear concretely in the synthesized world.
fn binary_functor_specializes_its_resource_methods() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
Import {pkg: "wavelet:coll/set" elem: s32 as: ints}
Export build
Def build Fn {} ints/new()"#,
    )
    .expect("a binary functor instantiation should specialize and synthesize");
    assert!(
        wit.contains("size: func() -> u32"),
        "specialized resource methods not emitted:\n{wit}"
    );
}

// ===========================================================================
// Phase E — tie-off: the worked example and the downstream surfaces
// ===========================================================================

// --- Step 12: worked example end-to-end + docs/examples regen ----------------

#[test]
#[ignore = "pending type system"]
// Step 12 — the `fig-source` program checks, monomorphizes, and synthesizes the
// `fig-wit` world: a concrete record, a derived `eq-point`, a `point`-specialized
// `Set` interface, and a fully concrete export — nothing generic survives.
fn worked_example_synthesizes_concrete_monomorphic_wit() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Import {pkg: "wavelet:coll/set" elem: point as: pts}
Export nearest-set
Def nearest-set Fn {ps: list(point)}
  Let {s: pts/new()}
    Do [ each(ps Fn {p} pts/add(s p))
         s ]"#,
    )
    .expect("the worked example should compile to a WIT world");
    assert!(wit.contains("record point"), "missing record point:\n{wit}");
    assert!(wit.contains("eq-point"), "missing derived eq-point:\n{wit}");
    assert!(wit.contains("point-set"), "missing specialized Set interface:\n{wit}");
    assert!(wit.contains("nearest-set"), "missing the export:\n{wit}");
    assert!(wit.contains("list<point>"), "export param not concrete:\n{wit}");
}
