# `produced-macros.wasm` — a Wavelet-produced macro component (Step 9)

Unlike `macros.wasm` (a hand-written Rust fixture for the *consumer* tests, Step
3), `produced-macros.wasm` is produced by the Step 9 **producer**: it is what
`wavelet build` emits from the Wavelet macro library `produced-macros.wvl`. It
bundles the Wavelet interpreter and the macro source as data (strategy A — see
`tools/macro-guest/README.md`).

It exports `wavelet:meta/macros@0.1.0` and publishes the macros in
`produced-macros.wvl`:

| macro      | arity | behaviour                                   |
|------------|-------|---------------------------------------------|
| `identity` | 1     | returns its single argument form unchanged  |
| `unless`   | 2     | `Unless c body` → `(if-MACRO c {} body)`    |

`tests/produced_macros.rs` consumes this checked-in binary so `cargo test` stays
hermetic (no wasm toolchain needed).

## Regeneration

Built without `wasm-tools`: `wavelet build` componentizes in-process. Refresh
the checked-in artifact when the guest (`tools/macro-guest`) or the producer
(`src/macrobuild.rs`) changes:

```console
# needs: rustup target add wasm32-unknown-unknown
cargo run -- build tests/fixtures/produced-macros.wvl -o /tmp/produced-out
cp /tmp/produced-out/demo-macros.wasm tests/fixtures/produced-macros.wasm

# optionally verify reproducibility through the producer API:
WAVELET_TEST_BUILD_MACRO_COMPONENT=1 cargo test --test produced_macros \
  reproduce_component_from_source
```
