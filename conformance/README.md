# roundtrip:suite — a WIT round-trip conformance suite

A wavelet-agnostic test of how much of the [WIT type
system](https://component-model.bytecodealliance.org/design/wit.html) a
component implementation can round-trip across a component boundary. It was
built to measure Wavelet's Component Model coverage, but nothing in it knows
about Wavelet: any implementation that can produce a component for the
`roundtrip` world can be swapped in as caller, callee, or both.

## The protocol

The WIT package (`wit/world.wit`, package `roundtrip:suite@0.1.0`) defines:

- **`values`** — one `*-rt` function per data type and function shape: every
  primitive, lists, options, all four `result` forms, tuples, records
  (including `%`-escaped field names), a variant, an enum, flags, a type
  alias, nested compounds, and multi-/zero-parameter functions.
- **`resources`** — a stateful `counter` resource with a constructor, methods,
  a static function, plus free functions passing owned and borrowed handles
  in both directions.
- **`runner`** — `run: func() -> result<_, list<string>>`, the entry point
  that drives every imported function and returns one message per failure.

Every function has the same contract: apply the **documented transform** to
the argument and return it (integers +1 wrapping, bool negated, string
appends `"!"`, enums rotate, flags toggle, compounds recurse, …). The
normative transform table lives in the doc comments of `wit/world.wit`.

An artifact is **symmetric**: it exports `values` and `resources` (so it can
be tested) and imports them while exporting `runner` (so it can test). Which
side is "under test" is chosen purely by composition.

### Seeds

The caller checks `response == transform(seed)` against its own hard-coded
seed table. Two builds with *different* seed tables are produced
(`roundtrip-a.wasm`, `roundtrip-b.wasm`) so a callee can't hard-code
responses — it must actually decompose and rebuild each value. Seed A uses
unremarkable values; seed B leans on edges (type maxima that must wrap, empty
strings/lists, `none`/`err` sides, supplementary-plane and maximal chars).

### Composition

Because artifacts are symmetric, the callee's own imports would dangle in a
naive two-component composition. The harness therefore composes a chain,
terminated by `stub.wasm` — an exports-only build whose functions all trap
and are never called:

    caller.wasm ∘ (callee.wasm ∘ stub.wasm)

Both `wac plug` steps and the final `wasmtime run --invoke 'run()'` are
driven by `test.sh`.

## Usage

```console
./test.sh                          # self-test: rust-a <-> rust-b, both directions
./test.sh CALLER.wasm CALLEE.wasm  # one directed pairing of two artifacts
```

To assess another implementation, build its component for the `roundtrip`
world and run it in both roles against both seed builds:

```console
./test.sh dist/roundtrip-a.wasm  yours.wasm   # can it be called correctly?
./test.sh dist/roundtrip-b.wasm  yours.wasm
./test.sh yours.wasm  dist/roundtrip-a.wasm   # can it call correctly?
./test.sh yours.wasm  dist/roundtrip-b.wasm
```

(Seeds only matter on the caller side, so the two callee-direction runs are
the ones the two seed builds exist for.)

## Layout

- `wit/world.wit` — the package; doc comments are the normative spec.
- `suite/` — the symmetric Rust implementation (`seed-b` cargo feature
  selects the second seed table).
- `stub/` — the exports-only chain terminator.
- `test.sh` — build + compose + run.

## Requirements

`cargo-component`, `wac`, `wasmtime` (with component `--invoke` support),
`wasm-tools`, and the `wasm32-wasip1` Rust target.
