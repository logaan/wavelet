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

#[test]
// Step 8 (§3 review fix) — overload mangling is only triggered by genuinely
// overloadable operations or a real ≥2-member overload set. An ordinary library
// name (`get`, `head`, `map`, …) defined once is *not* an overload: it keeps its
// given name at the boundary rather than being mangled into a degenerate label.
fn lone_library_named_def_is_not_mangled() {
    let wit = synth(
        r#"Package "demo:util@0.1.0"
Def get Fn {xs: list(s32)} xs
Export get"#,
    )
    .expect("a lone library-named export should synthesize");
    assert!(wit.contains("get: func("), "expected an un-mangled `get`:\n{wit}");
    assert!(
        !wit.contains("get-"),
        "lone `get` was wrongly name-mangled:\n{wit}"
    );
}

#[test]
// Step 8 (§3 review fix) — a mangled suffix derived from a constructor (generic)
// first-parameter type must be a legal WIT kebab identifier: `type_text` emits
// `list<s32>` whose `<`/`>` are illegal in a WIT name, so the synthesizer must
// sanitize it to `list-s32`. An intended `eq` overload over `list(s32)` and
// `string` therefore lowers to `eq-list-s32` and `eq-string`, with no `<`/`>`.
fn mangled_constructor_label_is_identifier_safe() {
    let wit = synth(
        r#"Package "demo:util@0.1.0"
Def eq Fn {a: list(s32) b: list(s32)} true
Def eq Fn {a: string b: string} true
Export eq"#,
    )
    .expect("constructor-typed overload set should synthesize");
    // The mangled function *label* must be a legal WIT identifier — no `<`/`>`.
    // (Parameter *types* like `list<s32>` legitimately use `<`/`>`; those are WIT
    // type syntax, not identifiers, so the check targets the `<label>: func(`.)
    assert!(
        wit.contains("eq-list-s32: func("),
        "expected identifier-safe label eq-list-s32:\n{wit}"
    );
    assert!(
        !wit.contains("eq-list<") && !wit.contains("eq-list>"),
        "mangled label contains illegal identifier characters:\n{wit}"
    );
    assert!(
        wit.contains("eq-string: func("),
        "missing mangled label eq-string:\n{wit}"
    );
}

#[test]
// Step 8 (§5 review fix) — first-parameter-only mangling collides when two
// members differ only *past* the first parameter: both `{a: point b: string}`
// and `{a: point b: s32}` would mangle to `eq-point`, synthesizing two functions
// of the same name into one interface (invalid WIT). The synthesizer must detect
// the collision and disambiguate over all parameter types, yielding distinct
// labels `eq-point-string` and `eq-point-s32`, with no duplicated function name.
fn first_parameter_mangle_collision_is_disambiguated() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Def eq Fn {a: point b: string} true
Def eq Fn {a: point b: s32} true
Export eq"#,
    )
    .expect("colliding overload set should synthesize over all parameter types");
    assert!(
        wit.contains("eq-point-string: func("),
        "expected disambiguated label eq-point-string:\n{wit}"
    );
    assert!(
        wit.contains("eq-point-s32: func("),
        "expected disambiguated label eq-point-s32:\n{wit}"
    );
    // No duplicated function name: the collided `eq-point` label must not survive
    // as a standalone declaration.
    assert!(
        !wit.contains("eq-point: func("),
        "collided first-parameter label leaked a duplicate function:\n{wit}"
    );
}

#[test]
// Step 8 (§5 review fix) — two members with *byte-identical* parameter type
// lists are a genuine duplicate definition: even the full-signature labels
// collide, so the set is unrepresentable in WIT. The synthesizer must report a
// clear compile error naming the export rather than emitting invalid WIT.
fn true_duplicate_overload_definition_is_an_error() {
    let err = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Def eq Fn {a: point b: point} true
Def eq Fn {a: point b: point} false
Export eq"#,
    )
    .expect_err("a true duplicate overload must be rejected");
    assert!(
        err.contains("eq"),
        "duplicate-overload error should name the export `eq`: {err}"
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

#[test]
// Step 9 (§6 review fix) — `Derive` auto-emits a bare `Export eq-point` for each
// derived op, so an author who *also* writes that same `Export eq-point`
// explicitly declares it twice. The synthesizer must collapse the identical
// declarations and emit exactly one `eq-point: func(...)`, not a duplicate WIT
// function.
fn derive_auto_export_and_explicit_reexport_dedup() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Export eq-point"#,
    )
    .expect("derive auto-export colliding with an explicit re-export should synthesize");
    assert_eq!(
        wit.matches("eq-point: func(").count(),
        1,
        "duplicate eq-point function in synthesized WIT:\n{wit}"
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

#[test]
// §7 regression — functor classification is keyed on the *package*, not on the
// presence of an `elem:` field. An ordinary import whose package is not a known
// functor package must stay an ordinary import even when it happens to carry an
// `elem:` field; that unknown field is ignored, not hijacked into a functor
// instantiation (which used to hard-error `unknown functor package`).
fn ordinary_import_with_elem_field_is_not_a_functor() {
    let wit = synth(
        r#"Package "demo:main@0.1.0"
Import {pkg: "acme:widget/thing" elem: point as: w}
Export run
Def run Fn {} drop("hi")"#,
    )
    .expect("an ordinary import carrying `elem:` should not be read as a functor");
    assert!(
        wit.contains("import acme:widget/thing;"),
        "import not treated as ordinary:\n{wit}"
    );
}

// ===========================================================================
// Phase E — tie-off: the worked example and the downstream surfaces
// ===========================================================================

// --- Step 12: worked example end-to-end + docs/examples regen ----------------

#[test]
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

// ===========================================================================
// Regression: the gradual checker must not reject programs the interpreter
// runs (the "never preempt a runtime success" invariant). Each of these was a
// false positive in the first cut of `check.rs`.
// ===========================================================================

#[test]
// `min`/`max` dispatch through the interpreter's `compare`, which is defined
// over strings and chars as well as numbers — so they must NOT be modelled as
// numeric-only. `min("a" "b")` runs and yields "a".
fn min_max_on_strings_is_not_rejected() {
    let r = run(r#"min("a" "b")"#);
    assert!(r.ok, "min on strings should run, got: {}", r.error);
    assert_eq!(r.value, r#""a""#);
}

#[test]
// The interpreter only conformance-checks a bare `Sym` `The` annotation; a
// constructor annotation like `list(s32)` is never checked, so the checker must
// stay gradual there rather than element-checking the list.
fn the_with_a_constructor_annotation_is_not_element_checked() {
    let r = run(r#"The list(s32) ["a"]"#);
    assert!(r.ok, "The list(s32) [\"a\"] should run, got: {}", r.error);
    assert_eq!(r.value, r#"["a"]"#);
}

#[test]
// `len` returns a plain Int that range-checks against any int type at runtime,
// so it must be modelled as an unconstrained int literal, not concrete `s64`
// (which would reject `The u8 len(...)`).
fn the_narrow_int_of_a_len_result_is_not_rejected() {
    let r = run(r#"The u8 len([1 2 3])"#);
    assert!(r.ok, "The u8 len(...) should run, got: {}", r.error);
    assert_eq!(r.value, "3");
}
