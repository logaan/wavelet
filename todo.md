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

### Plan & status — DONE, serving end to end

- [x] Vendor released WASI 0.2.0 WIT (io + clocks + http) → `src/wasi-http.wit`.
- [x] **WIT synthesis** (`wit.rs`, `emit.rs::synthesize_world_wit`): a
      `wasi:http/proxy` file emits a world that imports `wasi:http/types@0.2.0`
      (+ `wasi:io/streams`) and exports the external
      `wasi:http/incoming-handler@0.2.0`, with the vendored WIT appended.
      `wavelet wit` mirrors it.
- [x] **Resource types** (`emit.rs::WitTy::Handle`): `own<T>`/`borrow<T>` and the
      named wasi resources flatten to a single i32, wired through
      flat/size/align/lower/lift/store/load.
- [x] **`http/*` intrinsics** (`emit.rs::http_imports` / `http_call`): fields,
      outgoing-response, body, write (write+flush+drop stream), set, finish,
      path-with-query — each lowers to the canonical-ABI host import. The
      component encoder re-validates the signatures.
- [x] **Exported `handle`**: the existing export-wrapper path lifts the
      resource-typed params and lands the export in the external handler iface.
- [x] **End-to-end**: scaffold → build → compose → `wasmtime serve` → `curl`
      returns the page. Verified manually; `tests/http.rs` locks the build.
- [x] **Downstream surfaces**: CHANGELOG `[Unreleased]`, README status + http
      template blurb, `docs/docs/cli.mdx`. Lexer/grammars untouched (no new token
      classes). `cargo test` green.

### Remaining follow-ups (not blocking the http template)

- [ ] **Restore the counter template**: needs string/parsing builtins in the
      *wasm backend* — `split`, `reverse`, `read`, `to-s64` (they exist in the
      interpreter but not emit). Once present, the http template can read the
      `?count=N` query and become the original counter again.
- [ ] **Interpreter resources**: `wavelet run` of an http app can't execute
      (`http/*` are emit-only, no host). Either model host resources in the
      interpreter or have it report a clear "needs a host" message.
- [ ] **General resources**: only the fixed wasi:http intrinsic set is wired;
      user-defined resources / arbitrary resource methods are still unsupported.
- [ ] **error-code is hand-flattened**: `http/set` zero-fills the error-code
      slots from a hardcoded layout (the backend's `join_flat` refuses the
      i32/i64 widening a general variant needs). Fine while we only pass
      `ok(response)`; a general variant-flatten would remove the special case.

### Notes / decisions

- We do **not** `include wasi:http/proxy` (it drags in cli/random/sockets);
  `wasmtime serve` only needs a component that *exports*
  `wasi:http/incoming-handler@0.2.0`. So the target lowers to: import
  `wasi:http/types` (+ `wasi:io/streams` for the body), export the handler.
- The output-stream from `outgoing-body.write` is a child resource and must be
  dropped before `outgoing-body.finish`, or finish traps — so `http/write` is a
  composite that writes, flushes, and drops the stream.
