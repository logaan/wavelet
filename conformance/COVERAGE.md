# WIT-surface coverage matrix

The durable artifact of step 11 item 1 (LoT `lot:033yaDPS5llCTYg4SW5CkH`): one
row per WIT grammar feature, with the concrete test that pins each closed cell.
Every citation below was verified against main (`3d8f22c`) on 2026-07-31 —
including re-running the cheap ones — as part of the audit that produced this
file. Cells that are open say so explicitly.

## How to read the table

- **express** — a Wavelet component can *author* the feature: its synthesized
  WIT world uses it, and `wavelet build` produces a valid component.
- **consume** — a Wavelet component can *import* a dependency using the feature
  and interact with its values.
- **interp** — the reference interpreter (`src/interp.rs`, the semantics
  oracle) handles the feature's values.
- **backend** — the wasm backend (`src/emit/`) handles them, in agreement with
  the interpreter.
- **tested-by** — the concrete test / fixture / conformance check that pins the
  row. A row with no real citation says one of:
  - `GAP (item N)` — scheduled work; N is the step 11 work item (or another
    step, named). "in flight" = a parallel session is fixing it right now.
  - `REFUSED (…)` — deliberately rejected today, diagnostic quoted.
  - `deferred (step N)` — parked by decision on another step.

Conformance citations refer to the `roundtrip:suite` harness in this
directory: the caller role is `wavelet/src/runner.wlt` (55 checks: 45 `values`
+ 10 `resources`), the callee roles are `wavelet/src/roundtrip-values.wlt`
(all 36 `values` exports) and `wavelet/src/roundtrip-resources.wlt`, each run
against **both** rust seed builds via `wavelet/test-values-callee.sh` and
`wavelet/test-resources-callee.sh` (build the seeds first with
`conformance/test.sh`). Note: nothing runs these from `cargo test` or CI —
making conformance an automated gate is item 7.

## Primitives

| feature | express | consume | interp | backend | tested-by |
|---|---|---|---|---|---|
| bool | yes | yes | yes | yes | conformance `bool-rt` (both roles, both seeds); `tests/typed_scalars.rs::typed_bool_conditions_and_not` |
| u8 / u16 / u32 | yes | yes | yes | yes | conformance `u8-rt`/`u16-rt`/`u32-rt` + `every-primitive-rt`; `tests/typed_scalars.rs` (arithmetic, comparisons, conversions, traps); `tests/backend_byte_width.rs` (narrow widths at canonical stride/offset); `tests/result_threading.rs` (widening/narrowing rules) |
| s8 / s16 / s32 / s64 | yes | yes | yes | yes | conformance `s8-rt`..`s64-rt` + `every-primitive-rt`; `tests/typed_scalars.rs`; `tests/backend_byte_width.rs::signed_narrow_elements_sign_extend_in_guest` |
| u64 | **bit-carriage only (documented)** | same | same | same | conformance `u64-rt` passes both seeds, including seed B's `u64::MAX` (`conformance/suite/src/seeds.rs:138`) — but only because wrapping arithmetic mod 2^64 coincides in the i64 domain (`roundtrip-values.wlt:38`, `wrap-u64` is `add(n 1)`). Wavelet ints are s64: a literal above `i64::MAX` is a read error (verified: `9223372036854775808` → ``invalid integer literal``), and comparison/printing treat such values as negative. Pending the item 6 domain decision. |
| f32 | yes | yes | yes | yes | `tests/backend_f32.rs` (flat, record offsets, list stride, option payloads); conformance `f32-rt` both roles/seeds |
| f64 | yes | yes | yes | yes | conformance `f64-rt`; `tests/backend_numeric.rs::float_and_string_builtins_run_instead_of_trapping`; `tests/backend_to_string.rs::compiled_float_to_string_matches_the_interpreter` |
| char | yes | yes | yes | yes | `tests/backend_char.rs` (5 tests: boundary, guest identity, ordering, `to-char`, memory payloads); `tests/typed_scalars.rs::char_literals_match_as_patterns`; conformance `char-rt` with real next-scalar arithmetic incl. the surrogate gap (`roundtrip-values.wlt` `next-char`) |
| string | yes | yes | yes | yes | conformance `string-rt`, `list-string-rt`; `tests/backend_lists.rs` (string rebox at the seam, strings in canonical lists) |

## Built-in compounds

| feature | express | consume | interp | backend | tested-by |
|---|---|---|---|---|---|
| list | yes | yes | yes | yes | `tests/backend_lists.rs` (dep-born + constructed, ~16 tests); `tests/backend_byte_width.rs::narrow_list_elements_lift_at_canonical_stride`; conformance `list-u8-rt`, `list-string-rt`, `list-list-u8-rt` (nested list) |
| option | yes | yes | yes | yes | `tests/backend_variants.rs::option_cases_match_like_the_oracle`, `::option_export_takes_the_retptr_fast_path`; `tests/backend_byte_width.rs::option_u8_payload_sits_next_to_its_discriminant`; conformance `option-u8-rt` (both sides), `option-shape-rt` |
| option, nested (`option<option<T>>`) | yes | untested | yes | yes | **GAP (item 5)** — no in-tree test or conformance row anywhere. Works today: this audit built and ran an `option(option(u8))` round-trip export end-to-end (`some(some(4))` → `some(some(5))`, `some(none)` → `some(none)`); the finding is purely missing coverage. |
| result — bare, `result<T>`, `result<_, E>`, `result<T, E>` | yes | yes | yes | yes | `tests/payloadless_results.rs` (evaluate, WIT synthesis of absent arms, boundary crossing); `tests/result_threading.rs`; conformance `result-rt`, `result-u32-rt`, `result-string-err-rt`, `result-u32-string-rt`, `result-tuple-direction-rt`; the runner's own exported signature is `result<_, list<string>>` |
| tuple (`tuple2`..`tuple16`) | yes | yes | yes | yes | `tests/backend_tuples.rs`; `tests/tuple_constructors.rs` (constructors `tuple0`..`tuple16` as values, arity errors, first-class use); conformance `tuple-rt`, `tuple-nested-rt`, `multi-param` |
| `tuple1` in a boundary signature | yes | yes | yes | yes | Synthesizes valid `tuple<u8>`; full `wavelet build` verified in this audit. No dedicated in-tree test names a `tuple1` *signature* — covered indirectly by `tests/tuple_constructors.rs` value semantics; fixture welcome under item 5. |
| `tuple0` in a boundary signature | **no — fails late** | untested | yes (as a value) | interior only | **GAP (audit finding, this file; fix under item 5)** — `Def f Fn {} The tuple() tuple0()` synthesizes `tuple<>`, which `wasm-tools component wit` parses but componentization rejects: ``tuple type must have at least one type``. WIT forbids empty tuples, so the right outcome is a deliberate check-time refusal instead of the late internal error. Interior `tuple0` values are fine (`tests/tuple_constructors.rs`). |
| record | yes | yes | yes | yes | `tests/backend_records.rs` (~10 tests); `tests/type_system.rs::record_literal_field_type_mismatch_against_nominal_param_is_rejected` and neighbours; conformance `point-rt`, `every-primitive-rt`, `tuple-nested-rt` |
| record with `%`-escaped field names — consume | — | yes | yes | yes | conformance `awkward-rt` (`{record: 1 list: "r"}` against the dep's `%record`/`%list` fields), both seeds |
| record with `%`-escaped field names — express | **no — invalid WIT** | — | yes | — | **GAP (item 5)** — verified in this audit: `DefType awkward {%record: u32 %list: string}` synthesizes `record awkward { record: u32, list: string }` (the lexer strips the `%` — `src/lexer.rs:19` — and `src/wit.rs` never re-applies it), and `wavelet build` dies with ``internal: synthesized WIT did not parse: expected an identifier or string, found keyword `record` ``. |
| variant (incl. payload-less cases) | yes | yes | yes | yes | `tests/backend_variants.rs` (~17 tests); `tests/case_constructors.rs` (nullary + payloaded construction, arity/payload type errors, first-class constructors, dep cases, backend construction); conformance `shape-rt` (all four cases via `runner.wlt`) |
| variant — multi-payload case | **no — invalid WIT** | n/a (WIT deps can't declare one) | yes (bundles a tuple) | no | **GAP (item 2, in flight — no PR open as of 2026-07-31)** — verified in this audit: `DefType mylist [zip(list(u32) list(u32)) other]` synthesizes `zip(list<u32>, list<u32>)` (rendering site `src/wit.rs:1443`), which is invalid WIT; `wavelet build` fails late (``type `mylist` not supported by the wasm backend yet``) while the interpreter accepts and bundles (`tests/case_constructors.rs::multi_payload_case_bundles_a_tuple`). Planned fix: check-time rejection in both engines. |
| variant — case shadowing a builtin name (backend generic path) | yes | yes | yes | **diverges silently** | **GAP (item 3, in flight — fix open as [PR #42](https://github.com/logaan/wavelet/pull/42))** — `src/emit/call.rs:297` consults `BUILTINS` before `local_cases` (`:303`), the opposite precedence from the interpreter and checker. Repro from the step Thing: after `DefType t [not(u32) other]`, `to-string(not(5))` is `"not(5)"` interpreted but `"false"` compiled. PR #42 adds `tests/backend_variants.rs::local_case_shadows_builtin_on_the_boxed_path`; once merged this row's tested-by is that test. |
| variant / enum — >256 cases | see note | untested | yes | **REFUSED / partial** | **REFUSED** on the canonical memory path: ``variant with more than 256 cases is not supported by the wasm backend yet`` (`src/emit/canonical.rs:710`), ``enum with more than 256 cases …`` (`:719`). Verified in this audit, with a wrinkle the refusal misses: a 257-case **enum as a bare param/result never touches that path — it builds and runs correctly** (flat i32 discriminant); only nesting it (e.g. `list<big>`) refuses. A 257-case payloaded variant refuses at build. No in-tree boundary test at 256/257. Sizing-vs-limit is step 7 slice D's decision; item 6 records the outcome here with fixtures. |
| enum | yes | yes | yes | yes | `tests/case_constructors.rs` (incl. `::backend_constructs_enum_and_variant_cases`); enum-vs-variant synthesis split `src/wit.rs:1415`; conformance `direction-rt` + wrap check |
| flags (incl. local declaration + literals) | yes | yes | yes | yes | `tests/backend_flags.rs` (5 tests: round-trip, literal lowering, `eq`, pattern matching, canonical bit positions/alignment — local `DefType perms {read write exec}` at `tests/backend_flags.rs:21`; checker/synthesis support `src/check.rs:221-222`, `src/wit.rs:1412`); conformance `permissions-rt` both roles/seeds (complement expressed as a Match over all 16 declaration-order literals, `roundtrip-values.wlt`) |
| flags — 32-member boundary | yes (verified) | untested | yes | yes | 32 members builds **and runs** (verified in this audit); no in-tree test pins it — fixture belongs with item 6's boundary rows. |
| flags — >32 members | **no** | untested | yes | **REFUSED (late)** | **REFUSED**: Wavelet's own guard ``flags with more than 32 members is not supported by the wasm backend yet`` (`src/emit/canonical.rs:734`) fires only on the canonical memory path; a *flat* 33-member flags param escapes it and fails at componentize with wasm-tools' ``cannot have more than 32 flags`` (verified in this audit). Pending step 7 slice D. |
| type alias — of a dep type | — | yes | yes | yes | conformance `points-rt` (`type points = list<point>` from the dep), both roles/seeds |
| type alias — local, of a compound | yes | yes | yes | yes | `tests/backend_functor.rs:210` (`DefType nums list(s32)`); checker support `src/check.rs:223-224` |
| type alias — of a primitive; alias-of-alias | yes (verified) | untested | yes | yes | **GAP (item 5)** — no in-tree test. Works today: this audit built `DefType myint u32` + `DefType myint2 myint` through a full `wavelet build`, synthesizing `type myint = u32; type myint2 = myint;`. |

## Resources

| feature | express | consume | interp | backend | tested-by |
|---|---|---|---|---|---|
| resource declaration: constructor, methods, static | yes | yes | yes | yes | `tests/backend_resource.rs` (constructor/`next`/`value`, static `sum`); WIT synthesis pinned by `src/wit.rs:1633` (`synthesize_defresource_counter_block`); consume side `tests/generic_bridge.rs::generic_bridge_lowers_resource_methods_static_constructor_drop`; conformance `roundtrip-resources.wlt` via `test-resources-callee.sh` (both seeds) and `runner.wlt`'s 10 resource checks |
| own / borrow handles as bare params & results (both directions) | yes | yes | yes | yes | `tests/backend_resource.rs::own_and_borrow_free_functions_match_interpreter`, `::take_counter_consumes_and_reads`; `tests/generic_bridge.rs::generic_bridge_passes_resource_handles_own_borrow`; conformance `make-counter`, `bump-counter`, `take-counter`, `counter-round-trip`, `counter-to-point` |
| borrow in result / stored position — must be rejected (WIT forbids it) | rejected, unevenly | — | — | — | **GAP (item 5: pin the diagnostics; close the `The` hole)** — the deliberate rejections exist and fire (verified in this audit): `Export {… result: borrow(counter)}` → ``a `borrow(<resource>)` type cannot appear in result position`` (`src/check.rs:1284`); a `DefType` storing one → ``… cannot be stored in a `DefType` `` (`src/check.rs:1296`). But **no in-tree test cites either diagnostic**, and the `The borrow(counter)` result-annotation spelling escapes the check entirely, dying at componentize with ``internal: synthesized WIT did not parse: function `bad` returns a type which contains a `borrow<T>` ``. |
| handles in compound positions (`list<counter>`, `option<counter>`, record field, variant payload, `result<counter>`) | builds (verified) | untested | untested | untested | **GAP (item 5)** — nothing in `conformance/`, `tests/`, or `src/` exercises any of these at runtime. This audit verified `list(counter)` and `option(counter)` results *build*; runtime behavior on both engines and the consume side are unexercised. (Side finding: the resource constructor is not first-class — `map(counter starts)` is ``unbound name `counter` ``; a `Fn` wrapper works.) |
| two resource types in one interface; a method taking another resource | untested | untested | untested | untested | **GAP (item 5)** |

## Cross-package, interfaces, and worlds

| feature | express | consume | interp | backend | tested-by |
|---|---|---|---|---|---|
| cross-package type references in signatures | yes | yes | yes | yes | `tests/cross_package_sigs.rs` (dep types named in local export sigs and record fields); `tests/wit_deps.rs`; conformance `runner.wlt` (imports `types` + `values` + `resources`, exports `runner`) |
| package versions (`pkg:name@x.y.z`) | yes | yes | — | yes | `Package "conformance:wavelet@0.1.0"` + vendored `roundtrip:suite@0.1.0` (`conformance/wavelet/wit/deps/`, `wkg.lock`); wasi versioning `tests/wkg_populate.rs`; `tests/compose.rs::multi_component_composes_to_one` |
| `use` of dep/hoisted types in synthesized WIT | yes | yes | — | yes | `tests/cross_package_sigs.rs::local_record_fields_name_dep_types`; hoisted `types` interface + `use types.{…}` synthesis (`src/wit.rs:1194-1215`); `tests/type_system.rs::worked_example_synthesizes_concrete_monomorphic_wit` |
| synthesized interfaces: default `api`, hoisted `types`, functor-instantiated | yes | — | yes | yes | `tests/type_system.rs::functor_instantiation_synthesizes_concrete_interface`, `::two_functor_instantiations_make_two_interfaces`, `::worked_example_synthesizes_concrete_monomorphic_wit`; `tests/backend_functor.rs` (runtime agreement); steps 9/10 will add rows for user functors and dual-role imports citing *their* tests when they land |
| exporting into a foreign interface (`Export {iface: "pkg:ns/iface"}`) | yes | — | yes | yes | `tests/generic_bridge.rs::generic_bridge_exports_arbitrary_interface`, `::generic_bridge_exports_run_style_unit_result`; both conformance callees export into `roundtrip:suite/values` / `resources` |
| freestanding world-level functions | **inexpressible** | not consumed | — | — | Documented limitation: world synthesis only ever emits `import <iface>` / `export <iface>` lines (`src/wit.rs:1247-1277`); a func directly inside a `world` block can be neither authored nor (there is no import spelling for one) consumed. No scheduled item; raise a decision if a real-world WIT needs it. |
| whole-`roundtrip`-world symmetric export | not yet | — | — | — | **GAP (item 4)** — `conformance/wavelet/src/roundtrip.wlt` does not build (calls a nonexistent `flg` builtin, uses `map`/`filter` + untyped helpers pending step 3); its header and `conformance/wavelet/README.md` claim stale blockers. Owned by item 4, after step 3 gate D. |
| streams / futures | deferred | deferred | deferred | deferred | **deferred (step 8)** — no support anywhere in `src/` (the scaffolds' `wasi:io/streams` import is an ordinary resource-based dep, not the `stream<T>`/`future<T>` WIT types). |

## GAP inventory

The open cells above, in one list (the drift check in
`tests/coverage_matrix.rs` pins the ones that are cheaply observable):

1. **item 2 (in flight)** — multi-payload variant cases synthesize invalid WIT.
2. **item 3 (in flight, [PR #42](https://github.com/logaan/wavelet/pull/42))** —
   backend generic call path resolves builtins before local variant cases
   (silent divergence).
3. **item 4** — the symmetric `roundtrip.wlt` artifact does not build; its
   header and `conformance/wavelet/README.md` are stale (the README also says
   "35" values exports; there are 36).
4. **item 5** — untested-but-working: nested `option<option<T>>`,
   alias-of-primitive / alias-of-alias, 32-member flags, `tuple1` signatures,
   handles in compound positions (authoring builds; runtime + consume
   unexercised). Broken on the authoring side: `%`-escaped record field names
   (invalid WIT synthesized), `tuple0` signatures (invalid `tuple<>`, late
   failure). Unpinned diagnostics: borrow-in-result / borrow-in-`DefType`
   (plus the `The borrow(…)` spelling that escapes to a late internal error).
   Never exercised: two resources in one interface, a method taking another
   resource.
5. **item 6** — u64 stays "bit-carriage only (documented)" pending the domain
   decision; 256/257-case and 32/33-member boundary fixtures pending step 7
   slice D (note the flat-path escapes documented above).
6. **item 7** — nothing runs `conformance/` from `cargo test` or CI.
7. **step 8** — streams/futures.
