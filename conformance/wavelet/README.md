# Wavelet Component Model roundtrips

`src/roundtrip.wlt` combines the complete values and resources implementations
with the caller checks in one symmetric component. Run the full matrix from
any directory:

```sh
cargo build
WAVELET="$PWD/target/debug/wavelet" conformance/wavelet/test-matrix.sh
```

The harness builds the Rust A/B seed components and stub, validates the Wavelet
component, then checks Rust A → Wavelet, Rust B → Wavelet, Wavelet → Rust A,
Wavelet → Rust B, and Wavelet → Wavelet. Every pairing calls the full `run()`;
any build, composition, trap, or failed check makes the command fail.

The smaller `runner.wlt`, `roundtrip-values.wlt`, and
`roundtrip-resources.wlt` remain useful for isolating failures. Their two callee
scripts run only the corresponding interface slice.

Coverage includes all synchronous WIT value types in the suite, integer edge
wrapping, Unicode scalar boundaries, nested compounds, and resource ownership,
borrowing, constructors, methods and statics. It does not cover async/futures,
streams, threads or every experimental Component Model proposal. The normative
transform contract is in the parent suite documentation and WIT package.

Requirements: `cargo-component`, `wasm-tools`, `wac`, `wasmtime`, the Rust
`wasm32-wasip1` target, and the Wavelet binary (`WAVELET` overrides its path).
