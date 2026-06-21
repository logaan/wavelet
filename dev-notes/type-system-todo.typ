#set document(title: "Type system — implementation TODO")
#set text(font: "New Computer Modern", size: 10pt)
#set par(leading: 0.55em)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Type system — implementation TODO

Step-by-step worklist for `dev-notes/dd-type-system.typ` (monomorphic type
system). Each step is driven by failing tests in `tests/type_system.rs`, tagged
`// Step N`. Workflow per step: delete that step's `#[ignore]` lines, make the
tests green, keep `cargo test` passing.

Red baseline (all pending, all failing): `cargo test --test type_system -- --ignored`

Guiding split (`dd-type-system.typ` §6): _anything that must run during type
checking is core; anything expressible as a macro or a family of monomorphic
defs is standard library._

== Phase A — the core checker (the two rules, total monomorphic checking)

1. ☑ *Checker skeleton + literal types.* WIT type lattice; infer atom/literal
  types; unconstrained defaulting (`s64`/`f64`); wire a total check pass into the
  compile path (eval / run / build) so an ill-typed definition is rejected even
  when never called. \
  _tests:_ `illtyped_uncalled_def_is_a_compile_error`,
  `mixed_arithmetic_operands_are_rejected`

2. ☑ *Bidirectional checking of the core forms.* Propagate expected types inward;
  `If` branches and `Match` clauses must unify to one WIT type; `Let`/`Do`/`Fn`
  bodies checked; names resolved statically; every expression gets exactly one
  WIT type or it is a compile error (totality). \
  _tests:_ `if_branches_must_share_one_wit_type`,
  `match_clause_results_must_share_one_wit_type`,
  `unbound_name_in_uncalled_body_is_a_compile_error`,
  `let_binding_types_flow_into_the_body`

3. ☑ *`The` ascription + literal context-resolution & range checks.* Literals
  resolve to the expected type with a compile-time range check; `The` supplies an
  expected type to inference. \
  _tests:_ `out_of_range_literal_for_u8_is_a_compile_error`,
  `negative_literal_for_unsigned_is_a_compile_error`,
  `float_literal_where_int_expected_is_a_compile_error`

4. ☑ *Function signatures are WIT function types.* First-order and monomorphic;
  calls checked against the callee's signature (arity + argument types); a typed
  parameter pins its type in the body; signatures WIT cannot name are rejected. \
  _tests:_ `call_argument_type_mismatch_is_a_compile_error`,
  `call_arity_mismatch_is_a_compile_error`,
  `typed_parameter_used_at_wrong_type_is_rejected`

== Phase B — boundary synthesis from inference

5. ☑ *WIT synthesis driven by full inference.* Export signatures come from the
  checker's inferred types (un-annotated params inferred from use; concrete
  result types incl. literal defaults), replacing the best-effort `wit::infer`. \
  _tests:_ `synthesis_infers_untyped_parameter_from_use`,
  `synthesis_infers_list_result_type`

== Phase C — overloading (core: needs static types, so cannot be a library)

6. ☐ *Overload sets + argument-directed resolution.* Same-named monomorphic defs
  union into one overload set; resolve per call site by static argument types;
  equal applicability is a compile error (fixable by qualifying); imported sets
  union rather than conflict. \
  _tests:_ `overload_resolves_by_argument_type_first_definition`,
  `overload_resolves_by_argument_type_second_definition`,
  `ambiguous_overloaded_call_is_a_compile_error`

7. ☐ *Return-type-directed resolution.* When arguments don't decide, the expected
  type from `The` / surrounding context selects; no context ⇒ ambiguity error. \
  _tests:_ `return_type_directed_resolution_via_the`,
  `return_type_overload_without_context_is_a_compile_error`

8. ☐ *Boundary name-mangling for exported overload sets.* An exported overload
  set lowers to distinctly-named, concrete WIT functions (`eq-point`,
  `eq-string`); nothing generic reaches the boundary. \
  _tests:_ `exported_overload_set_is_name_mangled`,
  `mangled_overload_signature_is_concrete`

== Phase D — stdlib affordances, built on the core substrate

9. ☐ *`Derive` + derivers (Eq / Ord / Show / Hash).* Stdlib `tree → tree` macros
  emitting per-type monomorphic ops that join the overload sets; reader knows the
  `Derive` surface; derive-before-use ordering enforced. \
  _tests:_ `derive_eq_emits_a_monomorphic_definition`,
  `derived_eq_is_resolvable_and_synthesizes`

10. ☐ *Source functors via parameterized `Import`.* `Import {pkg … elem: t as: …}`
  instantiates a component per element type at compile time; required ops supplied
  by overload resolution; each instantiation synthesizes its own concrete
  interface (`point-set`, `string-set`); instantiate-before-use enforced. \
  _tests:_ `functor_instantiation_synthesizes_concrete_interface`,
  `two_functor_instantiations_make_two_interfaces`

11. ☐ *Binary-functor specialization (the one new core functor pass).* Monomorphize
  a precompiled, parameterized component by substituting the element type into its
  WIT (the case a macro cannot cover). \
  _tests:_ `binary_functor_specializes_its_resource_methods`

== Phase E — tie-off

12. ☐ *Worked example end-to-end + downstream surfaces.* The `fig-source` program
  checks, monomorphizes, and synthesizes the `fig-wit` world. Then sweep the
  downstream surfaces (`CLAUDE.md`): regenerate `docs/examples.json`
  (`./scripts/regen-examples.sh`), update docs prose, the three syntax grammars
  (lexer-derived), and the LSP for any new surface (`The`, `Derive`, functor
  `Import`, overloading diagnostics); add a `CHANGELOG.md` entry. \
  _tests:_ `worked_example_synthesizes_concrete_monomorphic_wit`
