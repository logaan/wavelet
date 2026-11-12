# Artifact size optimisation results

Measured on 12 November 2026. All sizes are uncompressed deployable component bytes.
Execution speed, compiler size, Rust firmware size, and runtime RAM are not measured.

## Sequential component sizes

| Workload | Baseline | 1. Binaryen | 2. Strip metadata | 3. Optional formatting | 4. Conditional ABI support | Total reduction |
|---|---:|---:|---:|---:|---:|---:|
| Minimal 1 + 1 | 6,225 | 1,082 | 697 | 470 | 222 | 96.43% |
| WASI hello world | 8,437 | 3,688 | 2,547 | 2,313 | 2,313 | 72.59% |
| Collections and strings | 6,733 | 4,342 | 3,929 | 3,929 | 3,929 | 41.65% |
| kb.wlt portfolio | 73,744 | 38,878 | 33,796 | 32,136 | 32,136 | 56.42% |

## Marginal savings from each change

Each row compares with the preceding checkpoint. These savings are sequential,
not independent estimates to apply to a different baseline.

| Change | Minimal | WASI hello | Collections | kb.wlt portfolio |
|---|---:|---:|---:|---:|
| Binaryen core optimisation | 5,143 | 4,749 | 2,391 | 34,866 |
| Metadata stripping | 385 | 1,141 | 413 | 5,082 |
| Conditional formatting support | 227 | 234 | 0 | 1,660 |
| Conditional ABI support | 248 | 0 | 0 | 0 |

1. **Binaryen:** `wasm-opt -Oz` optimises emitted core modules before componentisation. It owns dead-function elimination, inlining, and ordinary instruction simplification.
2. **Metadata:** size builds strip nonessential custom sections from final components. Separately composed artifacts use `wavelet compose --strip-metadata`. Public WIT interfaces remain unchanged.
3. **Formatting:** the compiler avoids generating its value printer and associated static strings when the expanded application has no `to-string` or `gensym` use. Macro libraries retain their compile-time printer. This is a conservative compiler-specific data decision, not a replacement for Binaryen reachability analysis. The collections fixture uses `to-string`, so its size is unchanged. A draft custom dead-code pass was reverted before measurement and contributes no reported savings.
4. **ABI support:** for suitable scalar-only components without imports, the compiler tries removing internal memory, allocator, and post-return support exports, reruns Binaryen, and accepts the result only when it has no remaining state/data/start obligations and preserves public function signatures. The minimal fixture qualifies; the other workloads do not.

## kb.wlt component breakdown

The project is a portfolio of six deployable component configurations, not one
monolithic application. The `led-dim` row is the composed dimmer and LED component;
its intermediate modules are not counted again. The standalone `led` row is a
separate configuration. Rust firmware and historical milestone copies are excluded.

| Component configuration | Baseline | Binaryen | Stripped | Optional formatting | Final |
|---|---:|---:|---:|---:|---:|
| braille | 11,308 | 5,974 | 5,237 | 5,000 | 5,000 |
| led | 8,161 | 3,420 | 2,846 | 2,616 | 2,616 |
| led-dim | 17,075 | 7,686 | 6,155 | 5,693 | 5,693 |
| passthrough | 11,016 | 6,053 | 5,299 | 5,065 | 5,065 |
| shift | 11,860 | 6,805 | 6,083 | 5,830 | 5,830 |
| toucan-keymap | 14,324 | 8,940 | 8,176 | 7,932 | 7,932 |

## Default builds without Binaryen

Default builds preserve the existing toolchain requirements. Stages 1 and 2 do
not change default output. The printer/data decision in stage 3 also benefits
default builds; conditional ABI support in stage 4 requires the size pipeline.

| Workload | Baseline default | Final default | Final size build |
|---|---:|---:|---:|
| Minimal 1 + 1 | 6,225 | 2,947 | 222 |
| WASI hello world | 8,437 | 5,161 | 2,313 |
| Collections and strings | 6,733 | 6,733 | 3,929 |
| kb.wlt portfolio | 73,744 | 50,752 | 32,136 |

The much larger default-build gain from stage 3 includes printer code that
Binaryen had already removed in the size-build checkpoints. This overlap is why
its marginal size-build gain is comparatively small.

## Reproduction and validation

```sh
cargo build --release
python3 scripts/bench-artifact-size.py \
  --kb /path/to/kb.wlt \
  --stage final-size \
  --build-arg=--optimize-size \
  --compose-arg=--strip-metadata \
  --baseline benchmarks/artifact-size/results/00-baseline.json \
  --output /tmp/wavelet-final-size.json
```

`--optimize-size` is a native build option and requires Binaryen `wasm-opt` on
`PATH`. The portable compiler component does not invoke an external optimiser.
The benchmark runner also requires `wasm-tools` and `wasmtime`.

Every checkpoint validated all nine final binaries and compared fixed expected
outputs, public WIT hashes, and all source/dependency hashes against baseline.
The initial baseline was repeated in independent temporary directories and all
nine deployment binaries were byte-identical. Smoke tests cover numeric output,
WASI stdout, empty/nonempty collection inputs, and representative keyboard/LED
operations. They do not claim exhaustive application correctness or hardware
validation. The full language/conformance suites are additional implementation
validation, separate from these artifact measurements.

The external project is read only and copied into temporary staging directories.
The reports record any optional post-build WIT-refresh warnings; those did not
prevent compilation, validation, or execution using the supplied WIT files.

| Checkpoint | Compiler source commit |
|---|---|
| Baseline | `2d63b5d9c43f01c0b0b4ce6243e9dc876dae34b2` |
| Binaryen | `df55eb8` |
| Metadata | `cb36313` |
| Formatting | `5287c43` |
| ABI support | `059de62` |

kb.wlt source revision: `15fe3512cb943edd654f3f7aba175c731efbe584` (clean at baseline).
Its six selected Wavelet source files total 272 lines. Source hashes are recorded
individually in the JSON reports.

- `wasm-tools`: wasm-tools 1.263.0
- `wasm-opt`: wasm-opt version 133
- `wasmtime`: wasmtime 50.0.1 (c307ec81d 2026-10-29)

Raw checkpoint data, section breakdowns, executable hashes, and smoke outputs
are in [results](results/). The benchmark methodology is described in
[README.md](README.md).
