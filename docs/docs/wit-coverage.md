---
title: WIT coverage
description: Audited WIT authoring, import, and runtime support, including boundary cases and known gaps.
---

# WIT coverage

Audited on **13 November 2026**, against Wavelet **0.12.1**, `wit-parser` / `wit-component` **0.251.0**, and Wasmtime **50.0.1**. This is a coverage report, not a claim of complete Component Model conformance.

**Author** means Wavelet can emit a validated component exposing the feature. **Import** means it can compile calls against a supplied WIT interface. The two execution columns distinguish a caller invoking Wavelet exports from Wavelet invoking an imported implementation. A successful import does not imply successful execution.

**Yes** means supported within the stated scope, **Partial** means a known restriction, **No** means unsupported or a demonstrated failure, and **Unverified** means execution has not been established by this audit. **Invalid** identifies a WIT restriction rather than missing Wavelet support. Runtime evidence comes from compiled components, never the compile-time interpreter. Host calls exercise the canonical ABI; they are identified separately from component-to-component evidence below.

## Core synchronous WIT

This table covers the baseline synchronous type system and interface structure. The Component Model itself remains under standardization; “core” here means the established WIT baseline, not Core WebAssembly instructions. See the upstream [WIT reference](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md).

| Feature | Author | Import | Caller → Wavelet | Wavelet → import | Limitations |
|---|---|---|---|---|---|
| `bool` | Yes | Yes | Yes | Yes | |
| `s8`, `s16`, `s32`, `s64` | Yes | Yes | Yes | Yes | |
| `u8`, `u16`, `u32`, `u64` | Yes | Yes | Yes | Yes | |
| `f32`, `f64` | Yes | Yes | Yes | Yes | |
| `char`, `string` | Yes | Yes | Yes | Yes | |
| Variable-length lists | Yes | Yes | Yes | Yes | |
| Records | Yes | Yes | Yes | Yes | |
| Nonempty tuples | Yes | Yes | Yes | Yes | |
| Options | Yes | Yes | Yes | Yes | |
| Results | Yes | Yes | Yes | Yes | |
| Enums, up to 256 cases | Yes | Yes | Yes | Yes | |
| Variants, up to 256 cases | Yes | Yes | Yes | Yes | |
| Flags, 1–32 members | Yes | Yes | Yes | Yes | |
| Nested non-resource compounds | Yes | Yes | Yes | Yes | |
| Primitive aliases and aliases of aliases | Yes | Yes | Yes | Yes | Alias names may change in generated WIT. |
| Compound aliases | Yes | Yes | Yes | Partial | Not all imported alias chains verified. |
| Cross-interface `use`, renamed types, transitive types | Partial | Yes | Partial | Partial | Not all re-export arrangements verified. |
| Resource declarations | Yes | Yes | Yes | Yes | |
| Resource constructors, methods, statics | Yes | Yes | Yes | Partial | Imported execution not fully verified. |
| Owned resource parameters and results | Yes | Yes | Yes | Yes | Owned handles cannot be passed to local functions. |
| Borrowed resource parameters | Yes | Yes | Yes | Partial | Imported execution not fully verified. |
| Owned resources inside lists, records, variants, options | Yes | Yes | **No** | Yes | Creating handles inside exported compounds traps. |
| Zero parameters, no result | Yes | Yes | Yes | Yes | |
| Multiple parameters; compound results | Yes | Yes | Yes | Yes | |
| Named interfaces and multiple export interfaces | Yes | Yes | Yes | Yes | |
| Implementing an external interface | Yes | Yes | Yes | Yes | |
| Packages, versions, escaped WIT identifiers | Yes | Yes | Yes | Yes | Dependencies must resolve to one version per package in a build. |
| Arbitrary world adoption (`include`, `with`, world-level functions, inline interfaces) | No | Partial | Unverified | Unverified | Only interface imports and exports determine the generated world. |

### Boundaries and ownership

| Case | Current result |
|---|---|
| 257 or more enum/variant cases | Dependency resolution can represent them, but canonical memory stores reject them. Flat-only paths are not a general support guarantee. |
| 33 or more flag members | Canonical memory stores reject them; do not rely on flat-only signatures for larger flag sets. |
| Empty tuple in an exported signature | Rejected during componentization: `tuple type must have at least one type`. Empty tuples are not valid WIT interface values. |
| Empty records, enums, variants, or flags | No coverage claim; this audit does not establish support for zero-member declarations. |
| Borrowed resource in a function result, including nested results | Invalid Component Model lifetime contract. Wavelet rejects tested direct and list-nested exported borrows. |
| Borrow stored in an authored record or variant type | Wavelet rejects it. This is stricter than merely prohibiting borrowed results: a named compound used only as a parameter is not necessarily an escaping borrow. |
| Borrowed resources nested in imported parameter compounds | Unverified; do not infer support from direct borrowed parameters. |
| Returning a newly created resource inside a list, record, variant, or option | Validated component, then an `unreachable` trap on invocation. Direct resource results work. |
| Receiving an imported owned-resource compound and transferring it back | Executed successfully for lists, records, variants, and options; the host receives the original handle representation. |
| Same-component calls taking owned handles | Rejected; use borrowed local parameters. Boundary ownership transfer is supported. |
| Resource aliases and unrelated same-named resources | Alias identity is retained; unrelated resources remain distinct and cannot be interchanged. |
| Dropping resources | Use resource destructor bindings for imported handles. Generic `drop(value)` discards a value without invoking a resource destructor. Automatic recursive cleanup of arbitrary compounds is not established here. |

The resource-compound failure is recorded as a known limitation, not hidden behind a compile-only success. The tests intentionally assert the current trap so that a fix also requires updating this report.

## Later released developer-preview features

Unsupported does **not** mean proposed. The upstream [release milestones](https://github.com/WebAssembly/component-model#milestones) list concurrency in 0.3.0 and maps plus interface annotations in 0.3.1. Those releases are separate from Wavelet's baseline coverage.

| Feature | Upstream grouping | Author | Import | Execute |
|---|---|---|---|---|
| `async func`, including async resource functions | 0.3.0 concurrency | No | No | No |
| `future<T>`, `future` | 0.3.0 concurrency | No | No | No |
| `stream<T>`, `stream` | 0.3.0 concurrency | No | No | No |
| Error contexts | Concurrency machinery | No | No | No |
| `map<K, V>` | 0.3.1 | No | No | No |
| `implements` / `external-id` interface annotations | 0.3.1 | No | Unverified | Unverified |

The dependency resolver explicitly rejects async functions, futures, streams, maps, and error contexts. Wavelet's language-level maps and map operations do not imply support for the WIT `map` type. Unsupported declarations can prevent resolution of a package even when the application does not call the affected function.

## Gated or proposed extensions

The upstream [gated-feature list](https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#gated-features) and WIT grammar also describe extensions beyond those released groups. Their presence in a parser or design document is not evidence that Wavelet supports them.

| Feature | Author | Import | Execute |
|---|---|---|---|
| Fixed-length `list<T, N>` | No | No | No |
| Resource getters and setters | No | Unverified | Unverified |
| Cooperative threads / shared-everything concurrency | No | No | No |
| Optional exports | No | Unverified | Unverified |

This is not an inventory of every Core Wasm proposal; SIMD, memory64, and other core instruction extensions are separate from WIT coverage.

## Evidence and reproduction

The ongoing [`tests/wit_coverage.rs`](https://github.com/logaan/wavelet/blob/main/tests/wit_coverage.rs) suite adds executable checks for nested options and compounds, primitive alias chains in both authored and imported signatures, 256/257 enum and variant cases, flag storage widths, empty tuple rejection, borrowed result/storage restrictions, and the directional resource-compound results above.

Its host-backed forwarding tests cross both Wavelet export and import boundaries and compare concrete values. A separate hand-authored component test composes **external caller → Wavelet → external callee**, checking `u32` values including the maximum. This establishes independent component interoperability for that signature, not a full cross-language matrix for every table row. Existing `backend_*`, `payloadless_results`, `generic_type_identity`, and `external_resource_exports` tests provide additional execution evidence; `generic_bridge` and parts of `generic_wit` provide compile-only evidence.

Run the added suite with `cargo test --test wit_coverage`; run the normal native suite with `cargo test`.

The broader Rust/Wavelet conformance harness is **not currently passing evidence**: `conformance/wavelet/test-matrix.sh` passed its Rust A/B self-tests during this audit, then stopped building `src/roundtrip.wlt` with `source forms are only available at compile time inside macro bodies`. Its intended five-way matrix must be repaired and rerun before claiming comprehensive Rust ↔ Wavelet conformance. These results also do not establish browser/Jco coverage for every audited edge case.
