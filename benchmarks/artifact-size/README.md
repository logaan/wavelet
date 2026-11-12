# Compiled artifact size benchmarks

These benchmarks measure uncompressed deployable WebAssembly component bytes,
not execution speed, compiler size, Rust firmware size, memory use, or gzip size.
Each checkpoint runs the same sources, validates every final component, checks
its public WIT contract, and executes fixed expected-output smoke cases.

## Workloads

| Benchmark | Deployment artifact | Behaviour checked |
|---|---|---|
| Minimal | One component exporting `add(1 1)` | Returns `2` |
| WASI hello world | One WASI CLI component | Writes exactly `Hello, world!` and a newline to stdout |
| Collections and strings | One component mapping squares to strings and joining them | Returns `"1,4,9,16"` and handles an empty list |
| kb.wlt project | Sum of six deployable components from the user's project | Braille mappings, LED colours, dimming composition, event passthrough, shift modifiers, and keymap processing |

The kb.wlt benchmark includes `braille`, `led`, `led-dim`, `passthrough`, `shift`,
and `toucan-keymap`. Its aggregate represents the project's component portfolio,
including alternative device configurations, not a single firmware image or one
large composed program. `led-dim` is composed with `led`; only that composed
artifact counts for the `led-dim` entry. The standalone `led` entry represents its
separate configuration. Individual component sizes are retained in every report.
The six source files total 272 lines at the recorded baseline revision. Rust
firmware, historical milestone copies, checked-in old binaries, and build
intermediates are excluded. These are host smoke tests, not hardware validation.

The external kb.wlt checkout is read only. Source and WIT files are copied into a
fresh temporary directory for every run. All fixtures and dependency inputs are
hashed in the report. The WASI fixture vendors the two small `wasi:cli` interfaces
it uses and copies the existing checked-in `wasi:io@0.2.0` definitions. No registry
fetch is required to compile; the CLI may emit a warning from its optional WIT
refresh after a successful build. Those warnings are recorded separately.

## Running a checkpoint

Build the compiler being measured, then run from the Wavelet checkout:

```sh
cargo build --release
python3 scripts/bench-artifact-size.py \
  --kb /path/to/kb.wlt \
  --stage baseline \
  --output benchmarks/artifact-size/results/00-baseline.json
```

For subsequent checkpoints, use the checked-in baseline to enforce unchanged
sources, public interfaces, and behaviour:

```sh
python3 scripts/bench-artifact-size.py \
  --kb /path/to/kb.wlt \
  --stage stage1-size \
  --build-arg=--optimize-size \
  --baseline benchmarks/artifact-size/results/00-baseline.json \
  --output benchmarks/artifact-size/results/01-size.json
```

From the metadata-stripping checkpoint onward, also pass
`--compose-arg=--strip-metadata` to strip the final `led-dim` composition.
Omit both arguments for the default-build comparison. `--wavelet` selects a frozen
compiler executable; `--compiler-commit` records its source revision when that
is different from the current checkout. The report also records the executable
hash and versions of Wasmtime, wasm-tools, and Binaryen. All three tools must be
on `PATH`.

## Measurement definitions

`benchmark_bytes` is the headline deployed-component size. `artifacts` retains
per-component deployment size, executable hash, WIT hash, input hashes, and smoke
results. `emitted_units_bytes` is the sum of individual compiler outputs excluding
`app.wasm`; it is diagnostic and is not added to the deployed size.

Section accounting recurses through nested components and core modules.
`core_bytes` includes each embedded core module in full; code/data/custom section
counts include their section framing. These are overlapping breakdowns, not
columns to add together. Custom sections are counted throughout the component,
including nested modules. The difference between final bytes and core bytes
includes component types, wiring, canonical adapters outside those modules, and
component metadata.

The baseline was captured before any of the four production changes, using a
frozen release compiler from commit `2d63b5d`. Later measurements are sequential:
core size optimisation, final metadata stripping, conditional printer/data emission,
and conditional allocation machinery. Binaryen owns general function reachability
and instruction optimisation; the compiler does not duplicate those passes. Report marginal savings against the
previous size-enabled checkpoint and cumulative savings against baseline.
Default-build measurements show improvements masked by an existing optimiser.

See [RESULTS.md](RESULTS.md) for measured sequential and marginal savings.
