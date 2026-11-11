use wavelet::{EvalOutcome, eval_snippet, expand, print, read_file, wit};

fn run(src: &str) -> EvalOutcome {
    eval_snippet(src)
}

fn synth(src: &str) -> Result<String, String> {
    let (arena, roots) = read_file(src).map_err(|e| e.to_string())?;
    let (arena, roots) = expand::expand_file(arena, &roots, None)?;
    wit::synthesize(&arena, &roots)
}

fn expand_forms(src: &str) -> Result<Vec<String>, String> {
    let (arena, roots) = read_file(src).map_err(|e| e.to_string())?;
    let (arena, roots) = expand::expand_file(arena, &roots, None)?;
    Ok(roots.iter().map(|&r| print(&arena, r)).collect())
}

#[test]

fn illtyped_uncalled_def_is_a_compile_error() {
    let r = run(r#"Def bad Fn {} add("a" "b")"#);
    assert!(
        !r.ok,
        "expected a compile-time type error, got value {:?}",
        r.value
    );
}

#[test]

fn mixed_arithmetic_operands_are_rejected() {
    let r = run(r#"Def bad Fn {x: s32} add(x "y")"#);
    assert!(!r.ok, "mixed s32 + string add should not type-check");
}

#[test]

fn if_branches_must_share_one_wit_type() {
    let r = run(r#"Def f Fn {b: bool} If b 1 "s""#);
    assert!(!r.ok, "heterogeneous If branches must be rejected");
}

#[test]

fn match_clause_results_must_share_one_wit_type() {
    let r = run(r#"Def f Fn {b: bool} Match b [(true 1) (false "s")]"#);
    assert!(!r.ok, "heterogeneous Match clause results must be rejected");
}

#[test]

fn unbound_name_in_uncalled_body_is_a_compile_error() {
    let r = run(r#"Def f Fn {} nope"#);
    assert!(!r.ok, "unbound `nope` should be caught statically");
}

#[test]

fn let_binding_types_flow_into_the_body() {
    let r = run(r#"Def f Fn {} Let {n: 1} str-cat(n "x")"#);
    assert!(!r.ok, "s64 binding used as string should be rejected");
}

#[test]

fn out_of_range_literal_for_u8_is_a_compile_error() {
    let r = run(r#"Def f Fn {} The u8 300"#);
    assert!(!r.ok, "300 is out of range for u8 and must be rejected");
}

#[test]

fn negative_literal_for_unsigned_is_a_compile_error() {
    let r = run(r#"Def f Fn {} The u8 -1"#);
    assert!(!r.ok, "-1 cannot be a u8");
}

#[test]

fn float_literal_where_int_expected_is_a_compile_error() {
    let r = run(r#"Def f Fn {} The s32 1.5"#);
    assert!(!r.ok, "1.5 cannot be an s32");
}

#[test]

fn call_argument_type_mismatch_is_a_compile_error() {
    let r = run(r#"Def g Fn {x: s32} x
Def f Fn {} g("str")"#);
    assert!(
        !r.ok,
        "string argument to an s32 parameter must be rejected"
    );
}

#[test]

fn call_arity_mismatch_is_a_compile_error() {
    let r = run(r#"Def g Fn {x: s32} x
Def f Fn {} g(1 2)"#);
    assert!(!r.ok, "arity mismatch must be rejected");
}

#[test]

fn typed_parameter_used_at_wrong_type_is_rejected() {
    let r = run(r#"Def f Fn {x: s32} str-cat(x "!")"#);
    assert!(!r.ok, "s32 parameter used as string must be rejected");
}

#[test]

fn synthesis_infers_untyped_parameter_from_use() {
    let wit = synth(
        r#"Package "demo:shout@0.1.0"
Export shout
Def shout Fn {phrase} str-cat(upper(phrase) "!")"#,
    )
    .expect("a fully-inferable export should synthesize");
    assert!(
        wit.contains("phrase: string"),
        "param type not inferred:\n{wit}"
    );
    assert!(
        wit.contains("-> string"),
        "result type not inferred:\n{wit}"
    );
}

#[test]

fn synthesis_infers_list_result_type() {
    let wit = synth(
        r#"Package "demo:r@0.1.0"
Export rev
Def rev Fn {xs: list(s32)} reverse(xs)"#,
    )
    .expect("list result should be inferable");
    assert!(
        wit.contains("-> list<s32>"),
        "list result not inferred:\n{wit}"
    );
}

#[test]

fn overload_resolves_by_argument_type_first_definition() {
    let r = run(r#"Def show Fn {x: s32} "int"
Def show Fn {x: string} "str"
show(5)"#);
    assert!(r.ok, "overloaded call failed: {}", r.error);
    assert_eq!(r.value, r#""int""#, "resolved to the wrong overload");
}

#[test]

fn overload_resolves_by_argument_type_second_definition() {
    let r = run(r#"Def show Fn {x: string} "str"
Def show Fn {x: s32} "int"
show("hi")"#);
    assert!(r.ok, "overloaded call failed: {}", r.error);
    assert_eq!(r.value, r#""str""#, "resolved to the wrong overload");
}

#[test]

fn ambiguous_overloaded_call_is_a_compile_error() {
    let r = run(r#"Def f Fn {x: u8} "a"
Def f Fn {x: s32} "b"
f(0)"#);
    assert!(!r.ok, "an ambiguous overloaded call must be rejected");
}

#[test]

fn return_type_directed_resolution_via_the() {
    let r = run(r#"Def make Fn {} 1
Def make Fn {} "x"
The s64 make()"#);
    assert!(r.ok, "return-type-directed call failed: {}", r.error);
    assert_eq!(r.value, "1", "ascription should select the s64 overload");
}

#[test]

fn return_type_overload_without_context_is_a_compile_error() {
    let r = run(r#"Def make Fn {} 1
Def make Fn {} "x"
make()"#);
    assert!(
        !r.ok,
        "return-type overload with no context must be rejected"
    );
}

#[test]

fn exported_overload_set_is_name_mangled() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Def eq Fn {a: point b: point} true
Def eq Fn {a: string b: string} true
Export eq"#,
    )
    .expect("exported overload set should synthesize");
    assert!(
        wit.contains("eq-point"),
        "missing mangled name eq-point:\n{wit}"
    );
    assert!(
        wit.contains("eq-string"),
        "missing mangled name eq-string:\n{wit}"
    );
}

#[test]

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

fn lone_library_named_def_is_not_mangled() {
    let wit = synth(
        r#"Package "demo:util@0.1.0"
Def get Fn {xs: list(s32)} xs
Export get"#,
    )
    .expect("a lone library-named export should synthesize");
    assert!(
        wit.contains("get: func("),
        "expected an un-mangled `get`:\n{wit}"
    );
    assert!(
        !wit.contains("get-"),
        "lone `get` was wrongly name-mangled:\n{wit}"
    );
}

#[test]

fn mangled_constructor_label_is_identifier_safe() {
    let wit = synth(
        r#"Package "demo:util@0.1.0"
Def eq Fn {a: list(s32) b: list(s32)} true
Def eq Fn {a: string b: string} true
Export eq"#,
    )
    .expect("constructor-typed overload set should synthesize");

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

    assert!(
        !wit.contains("eq-point: func("),
        "collided first-parameter label leaked a duplicate function:\n{wit}"
    );
}

#[test]

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

#[test]

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

#[test]

fn functor_instantiation_synthesizes_concrete_interface() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Instantiate {pkg: "wavelet:coll/set" with: {elem: point} as: pts}
Export has
Def has Fn {p: point} pts/contains(pts/new() p)"#,
    )
    .expect("a functor instantiation should synthesize");
    assert!(
        wit.contains("point-set"),
        "no specialized point-set interface:\n{wit}"
    );
}

#[test]

fn two_functor_instantiations_make_two_interfaces() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Instantiate {pkg: "wavelet:coll/set" with: {elem: point} as: pts}
Instantiate {pkg: "wavelet:coll/set" with: {elem: string} as: strs}
Export demo
Def demo Fn {p: point s: string} pts/contains(pts/new() p)"#,
    )
    .expect("two functor instantiations should synthesize");
    assert!(wit.contains("point-set"), "missing point-set:\n{wit}");
    assert!(wit.contains("string-set"), "missing string-set:\n{wit}");
}

#[test]

fn binary_functor_specializes_its_resource_methods() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
Instantiate {pkg: "wavelet:coll/set" with: {elem: s32} as: ints}
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

fn functor_application_spelling_is_instantiate() {
    let err = synth(
        r#"Package "demo:main@0.1.0"
Import {pkg: "wavelet:coll/set" elem: s32 as: ints}
Export run
Def run Fn {} drop("hi")"#,
    )
    .expect_err("Import of a functor package must error");
    assert!(err.contains("Instantiate"), "{err}");

    let err = synth(
        r#"Package "demo:main@0.1.0"
Instantiate {pkg: "wavelet:coll/set" with: {element: s32} as: ints}
Export run
Def run Fn {} drop("hi")"#,
    )
    .expect_err("unknown functor parameter must error");
    assert!(err.contains("no parameter `element`"), "{err}");

    let err = synth(
        r#"Package "demo:main@0.1.0"
Instantiate {pkg: "wavelet:coll/set" as: ints}
Export run
Def run Fn {} drop("hi")"#,
    )
    .expect_err("missing with: must error");
    assert!(err.contains("with: {elem:"), "{err}");
}

#[test]

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

#[test]

fn worked_example_synthesizes_concrete_monomorphic_wit() {
    let wit = synth(
        r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Instantiate {pkg: "wavelet:coll/set" with: {elem: point} as: pts}
Export nearest-set
Def nearest-set Fn {ps: list(point)}
  Let {s: pts/new()}
    Do [ each(ps Fn {p} pts/add(s p))
         s ]"#,
    )
    .expect("the worked example should compile to a WIT world");
    assert!(wit.contains("record point"), "missing record point:\n{wit}");
    assert!(wit.contains("eq-point"), "missing derived eq-point:\n{wit}");
    assert!(
        wit.contains("point-set"),
        "missing specialized Set interface:\n{wit}"
    );
    assert!(wit.contains("nearest-set"), "missing the export:\n{wit}");
    assert!(
        wit.contains("list<point>"),
        "export param not concrete:\n{wit}"
    );
}

#[test]

fn min_max_on_strings_is_not_rejected() {
    let r = run(r#"min("a" "b")"#);
    assert!(r.ok, "min on strings should run, got: {}", r.error);
    assert_eq!(r.value, r#""a""#);
}

#[test]

fn the_with_a_constructor_annotation_is_element_checked() {
    let r = run(r#"The list(s32) ["a"]"#);
    assert!(!r.ok, "The list(s32) [\"a\"] must be a compile error");
}

#[test]

fn the_narrow_int_of_a_len_result_is_not_rejected() {
    let r = run(r#"The u8 len([1 2 3])"#);
    assert!(r.ok, "The u8 len(...) should run, got: {}", r.error);
    assert_eq!(r.value, "3");
}

fn check_src(src: &str) -> Result<(), String> {
    let (arena, roots) = read_file(src).map_err(|e| e.to_string())?;
    let (arena, roots) = expand::expand_file(arena, &roots, None)?;
    wavelet::check::check_program(&arena, &roots)
}

#[test]
fn def_call_result_type_flows_into_the_caller() {
    let r = run(r#"Def g Fn {} "s"
Def f Fn {} add(g() 1)"#);
    assert!(
        !r.ok,
        "string-returning def used as a number must be rejected"
    );
}

#[test]
fn recursive_def_call_still_checks() {
    let r = run("Def count-down Fn {n} If eq(n 0) \"liftoff\" count-down(sub(n 1))\ncount-down(3)");
    assert!(r.ok, "recursive def failed: {}", r.error);
}

#[test]
fn record_literal_field_type_mismatch_against_nominal_param_is_rejected() {
    let r = check_src(
        r#"Package "demo:rec@0.1.0"
DefType point {x: s32 y: s32}
Def f Fn {p: point} 1
Def g Fn {} f({x: "a" y: 2})"#,
    );
    assert!(
        r.is_err(),
        "string field for s32-typed record field must be rejected"
    );
}

#[test]
fn record_literal_matching_nominal_param_is_accepted() {
    let r = check_src(
        r#"Package "demo:rec@0.1.0"
DefType point {x: s32 y: s32}
Def f Fn {p: point} 1
Def g Fn {} f({x: 1 y: 2})"#,
    );
    assert!(r.is_ok(), "well-typed record literal rejected: {r:?}");
}

#[test]
fn variant_ctor_payload_type_is_checked() {
    let r = check_src(
        r#"Package "demo:var@0.1.0"
DefType ttl [days(u32) forever]
Def f Fn {} days("x")"#,
    );
    assert!(r.is_err(), "string payload for days(u32) must be rejected");
}

#[test]
fn variant_ctor_and_nullary_case_type_as_the_nominal_variant() {
    let r = check_src(
        r#"Package "demo:var@0.1.0"
DefType ttl [days(u32) forever]
Def pick Fn {b: bool} If b days(30) forever"#,
    );
    assert!(r.is_ok(), "both If branches are `ttl`: {r:?}");
}

#[test]
fn match_binds_variant_payload_at_declared_type() {
    let r = check_src(
        r#"Package "demo:var@0.1.0"
DefType ttl [days(u32) forever]
Def f Fn {t: ttl}
  Match t [
    (days(d) upper(d))
    (forever "f")]"#,
    );
    assert!(r.is_err(), "u32 payload used as a string must be rejected");
}

#[test]
fn match_binds_record_fields_at_declared_types() {
    let r = check_src(
        r#"Package "demo:rec@0.1.0"
DefType point {x: s32 y: s32}
Def f Fn {p: point}
  Match p [
    ({x: a y: b} upper(a))
    (other "no")]"#,
    );
    assert!(r.is_err(), "s32 field used as a string must be rejected");
}

#[test]
fn quote_types_as_tree_and_rejects_arithmetic() {
    let r = run("add(Quote foo 1)");
    assert!(!r.ok, "a tree is not a number; add must be rejected");
}

#[test]
fn quasi_unquote_holes_are_checked() {
    let r = run("Quasi add(Unquote(nope) 1)");
    assert!(!r.ok, "unbound name inside an Unquote hole must be caught");
}

#[test]
fn quasi_data_heads_are_not_value_checked() {
    let r = run("Let {name: Quote(ada)} Quasi greeting(Unquote(name))");
    assert!(r.ok, "quasi data head wrongly value-checked: {}", r.error);
}

#[test]
fn defmacro_template_body_is_checked() {
    let r = run("DefMacro bad {x} add(nope 1)");
    assert!(!r.ok, "unbound name in a DefMacro body must be caught");
}

#[test]
fn defmacro_params_are_trees_in_the_body() {
    let r = run("DefMacro bad {x} add(x 1)");
    assert!(
        !r.ok,
        "tree-typed macro parameter used as a number must be rejected"
    );
}

#[test]
fn synthesis_infers_option_result_type() {
    let wit = synth(
        r#"Package "demo:opt@0.1.0"
Export lookup
Def lookup Fn {k: s64} If gt(k 0) some(mul(k 10)) none"#,
    )
    .expect("option result should now be inferable");
    assert!(
        wit.contains("-> option<s64>"),
        "option result not inferred:\n{wit}"
    );
}

#[test]
fn synthesis_infers_result_type_from_both_arms() {
    let wit = synth(
        r#"Package "demo:res@0.1.0"
Export checked
Def checked Fn {k: s64} If gt(k 0) ok(k) err("nonpositive")"#,
    )
    .expect("result<s64, string> should now be inferable");
    assert!(
        wit.contains("-> result<s64, string>"),
        "result type not inferred:\n{wit}"
    );
}

#[test]
fn synthesis_infers_list_literal_result() {
    let wit = synth(
        r#"Package "demo:lst@0.1.0"
Export pair
Def pair Fn {n: s64} [n mul(n 2)]"#,
    )
    .expect("list<s64> literal result should be inferable");
    assert!(
        wit.contains("-> list<s64>"),
        "list literal result not inferred:\n{wit}"
    );
}

#[test]
fn synthesis_infers_nominal_variant_result() {
    let wit = synth(
        r#"Package "demo:var@0.1.0"
DefType ttl [days(u32) forever]
Export pick
Def pick Fn {b: bool} If b days(30) forever"#,
    )
    .expect("nominal variant result should be inferable");
    assert!(
        wit.contains("-> ttl"),
        "variant result not inferred:\n{wit}"
    );
}

#[test]
fn macro_generated_type_error_is_a_compile_error() {
    let r = run("DefMacro bad-add {} Quasi add(\"a\" \"b\")\nDef f Fn {} Bad-add()");
    assert!(!r.ok, "macro-generated ill-typed code must be rejected");
}

#[test]
fn macro_generated_well_typed_code_still_runs() {
    let r = run("DefMacro twice {x} Quasi mul(2 Unquote(x))\nTwice(21)");
    assert!(r.ok, "well-typed macro expansion failed: {}", r.error);
    assert_eq!(r.value, "42");
}

#[test]
fn match_on_bool_must_cover_both_literals() {
    let r = run(r#"Def f Fn {b: bool} Match b [(true 1)]"#);
    assert!(!r.ok, "bool Match missing `false` must be rejected");
    assert!(r.error.contains("missing case `false`"), "{}", r.error);
}

#[test]
fn match_on_bool_with_both_literals_is_total() {
    let r = run(r#"Def f Fn {b: bool} Match b [(true 1) (false 2)]
f(true)"#);
    assert!(
        r.ok,
        "two-clause bool-literal Match should be total: {}",
        r.error
    );
    assert_eq!(r.value, "1");
}

#[test]
fn match_on_option_must_cover_none() {
    let r = run(r#"Def f Fn {} Match some(1) [(some(x) x)]"#);
    assert!(!r.ok, "option Match missing `none` must be rejected");
    assert!(r.error.contains("missing case `none`"), "{}", r.error);
}

#[test]
fn match_on_int_requires_catch_all() {
    let r = run(r#"Def f Fn {} Match 5 [(1 "one")]"#);
    assert!(!r.ok, "int Match without catch-all must be rejected");
    assert!(r.error.contains("catch-all"), "{}", r.error);
}

#[test]
fn match_with_catch_all_is_total() {
    let r = run(r#"Match 5 [(1 "one") (n "many")]"#);
    assert!(r.ok, "catch-all Match rejected: {}", r.error);
}

#[test]
fn match_on_deftype_variant_must_cover_every_case() {
    let r = check_src(
        r#"Package "demo:var@0.1.0"
DefType ttl [days(u32) forever]
Def f Fn {t: ttl} Match t [(days(d) d)]"#,
    );
    assert!(
        r.is_err(),
        "variant Match missing `forever` must be rejected"
    );
    assert!(
        r.as_ref().unwrap_err().contains("missing case `forever`"),
        "{r:?}"
    );
}

#[test]
fn refutable_subpatterns_do_not_count_as_coverage() {
    let r = run(r#"Def f Fn {} Match ok([1 2]) [(ok([]) 0) (err(e) 1)]"#);
    assert!(
        !r.ok,
        "refutable ok-subpattern must not count as covering `ok`"
    );
}

#[test]
fn node_type_table_types_param_refs_and_literals() {
    use wavelet::check::{self, Type};
    let (arena, roots) = read_file("Def double Fn {x: u32} add(x x)").expect("reads");
    let (arena, roots) = expand::expand_file(arena, &roots, None).expect("expands");
    let table =
        check::node_types_with_imports(&arena, &roots, &check::ImportSigs::new()).expect("checks");
    assert!(
        table.values().any(|t| *t == Type::U32),
        "the `x` param references should be typed u32: {table:?}"
    );
}

#[test]
fn node_type_table_propagates_check_errors() {
    use wavelet::check;
    let (arena, roots) = read_file("Def bad Fn {x: u32} add(x true)").expect("reads");
    let (arena, roots) = expand::expand_file(arena, &roots, None).expect("expands");
    assert!(check::node_types_with_imports(&arena, &roots, &check::ImportSigs::new()).is_err());
}

#[test]
fn let_bound_typed_closure_applies() {
    let r = run("Let {g: Fn {n: s32} add(n 1)} g(41)");
    assert!(r.ok, "typed closure application rejected: {}", r.error);
    assert_eq!(r.value, "42");
}

#[test]
fn function_def_reference_is_a_typed_value() {
    let r = run("Def inc Fn {n: s32} add(n 1)
Let {f: inc} f(5)");
    assert!(r.ok, "function-def-as-value rejected: {}", r.error);
    assert_eq!(r.value, "6");
}

#[test]
fn indirect_apply_type_mismatch_rejected() {
    let r = run("Let {g: Fn {n: s32} add(n 1)} g(true)");
    assert!(
        !r.ok,
        "type-mismatched indirect application must be rejected"
    );
    assert!(r.error.contains("mismatch"), "{}", r.error);
}

#[test]
fn indirect_apply_arity_mismatch_rejected() {
    let r = run("Let {g: Fn {n: s32} add(n 1)} g(1 2)");
    assert!(!r.ok, "wrong-arity indirect application must be rejected");
    assert!(r.error.contains("argument"), "{}", r.error);
}

#[test]
fn arrow_type_is_recorded_in_the_node_table() {
    use wavelet::check::{self, Type};
    let (arena, roots) = read_file("Let {g: Fn {n: s32} add(n 1)} g(1)").expect("reads");
    let (arena, roots) = expand::expand_file(arena, &roots, None).expect("expands");
    let table =
        check::node_types_with_imports(&arena, &roots, &check::ImportSigs::new()).expect("checks");
    let arrow = Type::Fn(vec![Type::S32], Box::new(Type::S32));
    assert!(
        table.values().any(|t| *t == arrow),
        "a typed Fn literal should carry the arrow (s32) -> s32: {table:?}"
    );
}

#[test]
fn unresolved_runtime_function_parameter_is_rejected() {
    let r = run("Def twice Fn {f x} f(f(x))\nDef inc Fn {n} add(n 1)\ntwice(inc 10)");
    assert!(!r.ok);
    assert!(
        r.error.contains("cannot infer a static type"),
        "{}",
        r.error
    );
}

#[test]
fn exported_function_result_rejected_at_synthesis() {
    let err = synth(
        "Package \"test:esc@0.1.0\"\n\nExport make-adder\nDef make-adder Fn {by: s32}\n  Fn {n: s32} add(n by)",
    )
    .expect_err("a function-valued export must be rejected");
    assert!(
        err.contains("function types cannot cross component boundaries"),
        "{err}"
    );
}

#[test]
fn exported_function_result_rejected_by_the_checker() {
    let src = "Package \"test:esc@0.1.0\"\n\nExport make-adder\nDef make-adder Fn {by: s32}\n  Fn {n: s32} add(n by)";
    let (arena, roots) = read_file(src).expect("reads");
    let (arena, roots) = expand::expand_file(arena, &roots, None).expect("expands");
    let err = wavelet::check::resolve_overloads(arena, &roots)
        .expect_err("the checker must reject a function-valued export");
    assert!(
        err.contains("function types cannot cross component boundaries"),
        "{err}"
    );
}

#[test]
fn internal_function_result_still_accepted() {
    let wit = synth(
        "Package \"test:ok@0.1.0\"\n\nExport shout\nDef make Fn {by: s32}\n  Fn {n: s32} add(n by)\nDef shout Fn {phrase: string} upper(phrase)",
    )
    .expect("non-exported closures must not trip the boundary rule");
    assert!(
        wit.contains("shout: func(phrase: string) -> string"),
        "{wit}"
    );
}
