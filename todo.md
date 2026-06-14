# todo.md — implementation tracking

## Active: WASI HTTP support — make the `http` project template actually work

Goal: `wavelet new app --type=http && cd app && scripts/serve.sh` builds a
component that `wasmtime serve` accepts and serves the counter page.

The template (`scaffold.rs`, `app_wvl`) needs:
- `Target "wasi:http/proxy"` → export `wasi:http/incoming-handler`.
- `Import {pkg: "wasi:http/types@0.2.0" as: http}` → an *external* WASI import.
- Resource-typed values: `incoming-request`, `response-outparam`,
  `outgoing-response`, `fields`, `outgoing-body`, `output-stream`.
- `http/*` calls mapping to `wasi:http/types` constructors / methods / statics.

### Plan & status

- [x] Vendor released WASI 0.2.0 WIT (io + clocks + http) as a single
      nested-package file `src/wasi-http.wit`; confirm it parses and a world
      importing `wasi:http/types` + exporting `wasi:http/incoming-handler`
      selects. (wit_parser round-trips it.)
- [ ] **WIT synthesis** (`wit.rs`, `emit.rs::synthesize_world_wit`): when a file
      targets `wasi:http/proxy`, emit a world that imports `wasi:http/types@0.2.0`
      and exports `wasi:http/incoming-handler@0.2.0` (referencing the external
      interface, *not* a locally-defined one), and append the vendored WIT.
      `wavelet wit` should print a world that round-trips.
- [ ] **Resource types in the type system** (`emit.rs::WitTy` / `wit_ty` /
      `flat`): treat resource handles (`own<T>`/`borrow<T>` and the named wasi
      resources) as a single `i32`. Hook into `flat`/`flat_len`/`size_of`/`align`.
- [ ] **Imported `http/*` calls** (`emit.rs`): resolve a `http/foo` qualified call
      against `wasi:http/types` to the right canonical-ABI core import
      (`[constructor]…`, `[method]…`, `[static]…`) with a correct flat signature,
      including result-via-retptr lifting for `result<own<T>, …>` / `option<…>`.
- [ ] **Exported `handle`** (`emit.rs`): lift the `incoming-request` (borrow) and
      `response-outparam` (own) params; land the export in the external
      `wasi:http/incoming-handler` interface.
- [ ] **Interpreter** (`interp.rs`): model resources as opaque handles enough that
      `wavelet wit`/`build` don't break. (`wavelet run` of an http app can't truly
      execute without a host — document this.)
- [ ] **End-to-end**: `scripts/build.sh` produces `out/app.wasm`; `wasmtime serve`
      accepts it and `curl localhost:8080` returns the counter HTML.
- [ ] **Downstream surfaces** (per CLAUDE.md): CHANGELOG `[Unreleased]`, docs if
      the language gained syntax, lexer-derived grammars only if token classes
      changed (they shouldn't), `cargo test` / `regen-examples.sh`.

### Notes / decisions

- We do **not** `include wasi:http/proxy` (that drags in cli/random/sockets);
  `wasmtime serve` only requires a component that *exports*
  `wasi:http/incoming-handler@0.2.0`. So the target lowers to: import
  `wasi:http/types`, export the handler. Imports of streams/poll/clocks come in
  transitively through `use`.
