# Step 07 — Scaffold templates & example `.wvl` files

**Read `dev-notes/tuple-calls.md` (the index) first.** Rewrite every Wavelet
source file the project ships — the scaffold templates and the `examples/`
files — into the new tuple-call syntax. After this step `cargo test --test http`
(which scaffolds and builds the HTTP template through the emitter) must pass.

Files: `src/scaffold.rs` (the `*_wvl` template builders), `examples/main.wvl`,
`examples/shout.wvl`, and any other `.wvl` under the repo (`fd -e wvl`). Work on
`tuple-calls`, commit as you go, no PR. Depends on Steps 01–06.

## Conversion rules (apply mechanically to every `.wvl`)

1. **List/record call sugar is gone.**
   - `f[a b]`  ⇒ `f([a b])`     (positional args were a list payload)
   - `f{k: v}` ⇒ `f({k: v})`    (named args were a record payload)
   - Most std calls used the bracket form: `str-cat[…]` ⇒ `str-cat(…)`,
     `add[a b]` ⇒ `add(a b)`, `eq[x y]` ⇒ `eq(x y)`, `sub[n 1]` ⇒ `sub(n 1)`,
     `len(x)` is already paren (unchanged), etc. A bracketed positional call
     becomes a paren call with the same items: `str-cat[a b]` ⇒ `str-cat(a b)`
     (the args are spliced into the call tuple — you do **not** keep the inner
     brackets unless the argument really is a list value).
2. **Zero-arg calls:** `f[]` and `f()` both ⇒ `f()`. So `args[]` ⇒ `args()`,
   `gensym[]` ⇒ `gensym()`, `fresh-pair[]` ⇒ `fresh-pair()`.
3. **Genuine list/record *values*** keep their brackets/braces: `[1 2 3]`,
   `{name: "Ada"}` as standalone values are unchanged. Only the *call* sugar
   changes.
4. TitleCase special forms (`Def`, `Fn`, `If`, `Let`, `Match`, `Export`,
   `Import`, `Package`, `Target`, …) are spelled exactly as before.
5. A qualified call `sh/shout{phrase: x}` ⇒ `sh/shout({phrase: x})`.

## Known conversions

`examples/shout.wvl`:
```
// shout.wvl — compiles to demo:shout.wasm
Package "demo:shout@0.1.0"

Export shout
Def shout Fn {phrase: string}
  str-cat(upper(phrase) "!")
```

`examples/main.wvl`:
```
// main.wvl — compiles to demo:main.wasm
Package "demo:main@0.1.0"
Target "wasi:cli/command"

Import {pkg: "demo:shout/api" as: sh}

Export run
Def run Fn {}
  If eq(len(args()) 0)
     println("usage: main <word>")
     println(sh/shout({phrase: head(args())}))
```

`src/scaffold.rs` — `greeting_wvl` body:
```
Def greet Fn {{name: string}}
  str-cat("Hello, " name "!")
```
(Note: inside `format!` the literal braces are doubled `{{ }}`. The only change
is `str-cat[ … ]` ⇒ `str-cat( … )`.)

`main_wvl` and `app_wvl` (HTTP) templates: read each `format!` body and apply the
rules above. Typical edits: `str-cat[…]` ⇒ `str-cat(…)`, `args[]` ⇒ `args()`,
`println(...)` is already paren, any `name{…}`/`name[…]` call ⇒ `name({…})` /
`name([…])`. Read the current bodies carefully — convert every call, leave value
literals and special forms alone.

## Verification

- `cargo test --test http` passes (scaffolds the HTTP project and builds both
  components through the emitter with validation on).
- Manually build the CLI template too:
  `cargo run -- build` on a scaffolded CLI project (or call
  `wavelet::scaffold::create` + `wavelet::build::build_files` in a scratch dir)
  and confirm it compiles.
- Build the `examples/` files: `cargo run -- build examples/shout.wvl
  examples/main.wvl` (adjust to the actual CLI) must succeed.

## Commit

e.g. `feat(scaffold,examples): rewrite .wvl sources to tuple-call syntax`
