---
title: Compiler API
---

Wavelet's compiler accepts source text and WIT dependencies in memory and returns
WebAssembly components. The native CLI and browser REPL use this same compiler.
The browser executes the emitted component. Runtime semantics follow WebAssembly;
the evaluator is used only to execute macros during compilation.

## Rust API

`wavelet::compiler::compile(&Request, &mut dyn MacroHost)` returns
`Result<Step, Vec<Diagnostic>>`. It does not read files, launch processes, fetch
packages, or execute the resulting program.

A request contains:

| Field | Meaning |
| --- | --- |
| `sources` | Wavelet files, each with a virtual `name` and UTF-8 `contents`. |
| `dependencies` | Supplied WIT files, each with a virtual `name` and byte `contents`. |
| `entry` | Optional source filename to use as the project entry. |
| `mode` | `Project` for complete packages, `Snippet` for a final display value, or `Repl` for every top-level result. |

Virtual filenames use `/` separators. Empty names, absolute paths, backslashes,
empty path segments, `.` and `..` segments, and duplicate filenames are errors.
These names identify inputs and diagnostics; they grant no filesystem access.

Dependencies can be `.wit` text, encoded `.wasm` WIT packages or components, or
`.wat` text encodings. WIT fragments in the same directory form one package;
place separate packages in separate directories. Each root-level `.wit` file is
parsed as a separate package. Dependencies resolve from these supplied files and
other source packages. Missing packages produce diagnostics without a network
lookup.

A successful `Step::Complete` contains named artifacts, an optional entry artifact
name, and an optional snippet invocation. Artifact kinds distinguish ordinary
components, macro components, and a composed application. `entry_artifact()`
looks up the selected artifact. Project components are composed in memory.
Unresolved external runtime interfaces remain imports for the application host
to provide.

When `entry` is omitted, the compiler chooses an unimported runtime package,
preferring one that imports another supplied source package. Projects with several
independent roots should set `entry` explicitly. A project containing only macro
libraries returns artifacts without a runtime entry.

Diagnostics carry the failing phase, an optional source filename, an optional
byte span, and a message. Reader diagnostics include a byte position. Later stages
currently report the source filename without a span. A failure returns no partial
compilation.

For a snippet without user macros:

```rust
use wavelet::compiler::{compile, NoMacros, Request, Step};

let request = Request::snippet("add(20 22)");
let Step::Complete(compilation) = compile(&request, &mut NoMacros).unwrap() else {
    panic!("this snippet needs a macro host");
};
let bytes = &compilation.entry_artifact().unwrap().bytes;
```

Native applications can use `compiler::native::compile` to handle macro
preparation with Wasmtime. `request_from_paths` supplies the filesystem adapter
used by the native build command. These adapters are unavailable in the compiler
component.

## Snippet execution

Snippet mode accepts one source containing declarations and expressions. The
compiler preserves declarations, exports declared resources, and wraps the
expressions in an exported function. Its returned invocation currently names
`wavelet:snippet/api@0.1.0` and `evaluate`. Hosts should use the returned names.
The function returns the final value as a string in Wavelet's printed value
syntax. Declaration-only snippets return an empty string.

A snippet can define functions, macros, types, resources, and functor templates.
`Package`, `Target`, and explicit `Export` declarations belong in project mode.
The wrapper is shared by the native REPL, browser REPL, and documentation
conformance tests. Repeated evaluation executes compiled code again. A host that
retains earlier definitions resubmits them with the next expression.

`Mode::Repl` (component mode `repl`) accepts the same single-source input and
returns the same invocation names. Its function returns a list of records with
`line: u32` and `value: string`, one for each top-level form in source order.
Lines are one-based starting lines in the submitted source, measured before macro
expansion. Values use Wavelet's printed syntax; declarations return `{}` after
evaluating any value initializer. All forms execute within one invocation, so
mutations carry between expressions. Empty input returns an empty list. A runtime
failure aborts the invocation without returning partial results.

## Macro preparation

Macro execution is an explicit host capability. `Step::NeedsMacros` asks the host
to prepare libraries before compilation can continue. Each request has an opaque
key and either generated component bytes or an imported package reference with an
optional artifact path. A generated library can come from local `DefMacro`
declarations or a macro-library source in the same project. An explicit `from`
artifact takes precedence over a supplied source library.

The host instantiates each requested component, records it under its key, and
retries the same request with those capabilities. A library exposes synchronous
`manifest` and `expand` methods using `wavelet:meta/code@0.1.0` trees. The compiler
obtains manifests while reading, prepares all required capabilities before
expansion, and validates returned tree references and cycles before consuming
them. Library preparation may require several retries because reading an import
can reveal the arity needed to parse later forms.

Imported references describe artifacts the host must supply. They are not
permission to fetch a registry package. The native adapter resolves explicit
artifact paths and the project's conventional macro directory. The browser host
uses artifacts selected by the user. Browser component instantiation can happen
asynchronously between calls; each compiler call and macro method stays
synchronous.

The host must retain the same instantiated library across retries. Do not reset
its state when wrapping it for the next compiler call. Report manifest or
expansion failures as errors, and terminate compilation if a requested capability
cannot be provided.

## Component API

The checked-in WIT contract is
`components/compiler/wit/compiler.wit`, package `wavelet:compiler@0.1.0`, world
`compiler-component`. It exports the `compiler` interface and imports
`macro-host`, together with the shared tree types. It imports no WASI interfaces.

The exported `compile` function accepts a request and a list of macro bindings.
Each binding owns a `macro-host.library` resource for that call. Create fresh
resource wrappers on each call while retaining the underlying instantiated macro
library. Dropping a wrapper releases that call's binding; it must not discard the
host's retained library. This ownership rule avoids borrowing a resource through
a list of records and works with the native and JavaScript bindings.

The component returns the same `needs-macros` or `complete` steps as the Rust API.
A host compiles, satisfies preparation requests, instantiates the selected entry
artifact, and calls the supplied invocation. Runtime failures are separate from
compiler diagnostics. The browser worker can be terminated to cancel a long
compilation or execution.

Build the guest with `./scripts/build-compiler-component.sh`. The build uses the
`wasm32-unknown-unknown` target and component metadata from `wit-bindgen`; it does
not add a WASI adapter. The resulting
`docs/src/compiler/wavelet_compiler.wasm` is checked in so the documentation site
can build with Node alone. Regenerate it whenever compiler code or its WIT
contract changes.

## Native size optimisation

`compiler::native::compile_for_size(request, root)` and
`build::build_files_for_size(paths, out_dir)` use Binaryen's `wasm-opt -Oz` before
componentisation and require `wasm-opt` on `PATH`. Failures are diagnostics, not
silent fallbacks. `compiler::compile_with_optimizer` accepts an
`emit::CoreOptimizer` callback for hosts that provide their own core-Wasm
optimiser. It receives a core module before component metadata is embedded and
must return a semantically equivalent core module with the same ABI exports and
imports. The normal `compile` API and the compiler component remain self-contained;
the component protocol adds no optimiser host import.
