// review.todo.typ — compiler implementation review, as an actionable todo list.
// Render: `typst compile review.todo.typ` (or `typst watch review.todo.typ`).

#set document(title: "Wavelet compiler review", author: "Claude (Opus 4.8)")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#show link: set text(fill: rgb("#1f6feb"))
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)
#show heading.where(level: 2): set text(size: 11pt)

// ---- helpers ---------------------------------------------------------------

#let sevcolor = (high: rgb("#c0392b"), med: rgb("#ca6f1e"), low: rgb("#7f8c8d"))

#let badge(level) = box(
  fill: sevcolor.at(level),
  inset: (x: 5pt, y: 1.5pt),
  radius: 3pt,
  baseline: 0.12em,
  text(fill: white, size: 7.5pt, weight: "bold", upper(level)),
)

#let checkbox = box(
  width: 0.9em, height: 0.9em,
  stroke: 0.7pt + luma(45%), radius: 1.5pt,
  baseline: 0.15em,
)

#let at(loc) = raw(loc) // a `file:line` reference

#let finding(level, title, loc, body) = block(
  width: 100%, above: 1.0em, below: 0.4em, breakable: true,
  [
    #checkbox #h(0.5em) #badge(level) #h(0.55em) *#title*
    #block(inset: (left: 1.65em, top: 3pt), {
      text(size: 8.5pt, fill: luma(38%))[#loc]
      parbreak()
      body
    })
  ],
)

// ---- title -----------------------------------------------------------------

#block(
  fill: luma(96%), width: 100%, inset: 12pt, radius: 5pt,
  [
    #text(size: 17pt, weight: "bold")[Wavelet — compiler implementation review]
    #v(2pt)
    #text(size: 9pt, fill: luma(35%))[
      Scope: `read → expand → interpret → wit → emit → componentize` (≈13k LOC) ·
      Date: 2026-06-20 · Reviewer: Claude (Opus 4.8)
    ]
  ],
)

*Verdict.* High-quality, unusually disciplined codebase. Stage separation is clean
and matches the docs; the arena/`Node` model is simple and consistent; the
canonical-ABI machinery in `emit.rs` is careful and well-commented; the macro-table
collision policy and the wasm-safe foreign-macro seam are elegant and exhaustively
tested. `cargo test` is fully green. The items below are mostly edge cases, not
structural problems — the headline is a cluster of interpreter↔backend divergences
that the project's own contract classifies as bugs.

#block(inset: (y: 4pt), [
  #text(size: 8.5pt)[
    *Legend* #h(0.6em) #checkbox open todo #h(1.2em)
    #badge("high") compiles clean, wrong/trapping at runtime #h(1.0em)
    #badge("med") oracle mismatch, narrow trigger #h(1.0em)
    #badge("low") nit / cosmetic
  ]
])

= 1 · Interpreter ↔ wasm-backend divergences

`CLAUDE.md` states the contract: _"a wasm-backend change that diverges from the
interpreter is a bug."_ The backend's `builtin()` (#at("emit.rs:2721")) reimplements
the numeric builtins with raw i64 wasm ops, decoupled from the interpreter's
`arith` / `compare` / `args_n` discipline (#at("builtins.rs:88-115") , #at("builtins.rs:139")).
That decoupling produced three observable divergences.

#finding(
  "high",
  [Float / string arithmetic + comparison compiles clean but traps at runtime],
  [#at("emit.rs:2745-2784") · guard #at("emit.rs:3395-3409") · inference #at("wit.rs:484") , #at("wit.rs:506-512")],
  [
    `add/sub/mul/div/rem/neg` and `lt/le/gt/ge` emit `unbox_int` unconditionally, so a
    value-typed `f64` (or `string`/`char` for comparisons) traps where the interpreter
    returns a value. `f64` is fully plumbed through `lift/lower/store/load` and WIT
    inference happily infers it, so nothing rejects float arithmetic — it compiles with
    a valid signature and only fails at runtime. The design.md "errors at the edge" note
    is about _type_ errors; here both sides agree the program is well-typed and disagree
    on the result.

    _Verified._ Interpreter: `add(1.5 2.5)` → `4.0`, `lt(1.5 2.5)` → `true`,
    `lt("a" "b")` → `true`. Backend: `Def run Fn {x: f64} add(x x)` (result `f64`)
    *builds successfully*; its core body disassembles to `i64.add` (1×) with no `f64.add`
    (0×) — it unboxes the float as an integer and traps via the int-tag guard.

    ```sh
    $ wavelet build flt.wvl          # succeeds
    $ wasm-tools print out/demo-flt.wasm | grep -c 'i64.add'   # 1
    $ wasm-tools print out/demo-flt.wasm | grep -c 'f64.add'   # 0  ← never adds as float
    ```

    *Fix (smaller).* Reject at compile time what the backend can't faithfully implement —
    gate these builtins on the operand box tag (as `eq_raw` already dispatches) and emit a
    build error for non-integer operands, so the divergence surfaces at build, not as a
    runtime trap. *Fix (fuller).* Implement the float/compare paths.
  ],
)

#finding(
  "med",
  [Arithmetic is variadic in the backend, strictly binary in the interpreter],
  [#at("emit.rs:2759-2777") vs #at("builtins.rs:139")],
  [
    `add(a b c)` folds over `items[1..]` in the backend → `a+b+c`, but the interpreter
    requires exactly two args (`args_n(arg, 2)`). The backend silently accepts what the
    oracle rejects.

    _Verified._ Interpreter errors ```add expects 2 arguments```; the backend compiles
    `Def run Fn {n: s64} add(n n n)` without complaint.

    *Fix.* Make the backend reject arity ≠ 2 for these builtins (or teach the interpreter
    to fold) — pick one and make both agree.
  ],
)

#finding(
  "med",
  [Integer overflow wraps in the backend, errors in the interpreter],
  [#at("emit.rs:2759-2777") vs #at("builtins.rs:88-93") , #at("builtins.rs:139-143")],
  [
    The backend uses `I64Add/Sub/Mul` (2's-complement wrap) and `I64DivS/RemS` (trap on
    `0` and on `INT_MIN / -1`); the interpreter uses `checked_*` and returns catchable
    errors. So `add(i64::MAX 1)` errors in the interpreter but wraps in wasm, and
    `div(1 0)` errors in the interpreter but traps in wasm.

    *Fix.* Decide the language semantics (wrapping vs checked) and make both backends
    honour it; if checked, the wasm path needs an overflow/zero guard around the op.
  ],
)

= 2 · Lexer

#finding(
  "low",
  [`-inf` matches as a prefix, not a whole word],
  [#at("lexer.rs:94") (negative) vs #at("lexer.rs:155-160") (positive)],
  [
    The negative-infinity literal is matched with `src[i+1..].starts_with("inf")`, so any
    token *beginning* with `-inf` is split. The positive `inf`/`nan` path requires a
    whole-word match, so this is an asymmetry.

    _Verified._ `-info` → ```[-inf, o]```, `-infinity` → ```[-inf, inity]```, while bare
    `info` stays a single identifier. Severity is very low — identifiers can't start with
    `-`, and numeric lexing already doesn't enforce a trailing boundary (`-1abc` →
    `[-1, abc]`).

    *Fix.* Require the byte after `inf` to be a non-name char before emitting `-inf`.
  ],
)

= 3 · Smaller robustness notes (not bugs)

#finding(
  "low",
  [`check_type` doesn't range-check `u64`],
  [#at("interp.rs:521") vs the `to-u64` builtin #at("builtins.rs:338")],
  [
    ```rust "u64" | "s64" => matches!(v, Value::Int(_))``` accepts negatives for a `u64`
    parameter — the only unsigned type without a bound, and inconsistent with `to-u64`
    which checks `n >= 0`. Harmless under dynamic typing, but easy to align.
  ],
)

#finding(
  "low",
  [`def_wrapper_slot` skips the payload tag/arity guard],
  [#at("emit.rs:1220-1229") vs `fn_form` #at("emit.rs:1274-1287")],
  [
    The uniform wrapper reads the payload as a list (`I32Load @8+4i`) without the
    tag/length check `fn_form` emits. Safe for well-typed callers (payload comes from
    `payload_box`), but a malformed indirect call reads garbage rather than trapping.
  ],
)

#finding(
  "low",
  [`owner_for_alias` re-queries `manifest()` per lookup (perf only)],
  [#at("macrodep.rs:283") vs memoized `owner_of` #at("macrodep.rs:259")],
  [
    Qualified-call resolution re-runs the component's `manifest()` on every lookup,
    unlike the memoized bare-name path. Correctness-neutral; memoise if it shows up.
  ],
)

#finding(
  "low",
  [`run_files` silently no-ops a missing `run`],
  [#at("runner.rs:63-69")],
  [
    `wavelet run` does nothing (no error) when the entry file has no `run` closure. A
    diagnostic would be friendlier.
  ],
)

= 4 · What's notably good (no action)

- *Canonical-ABI handling* — `join_flat` / `flat_checked` / `variant_payload_offset` and
  the lower/lift/store/load chains (#at("emit.rs:280-520") , #at("emit.rs:1859-2719")) are careful and
  read as correct, including the variant payload-widening coercions.
- *Macro system* — the `MacroTable` ambiguity / qualified-resolution policy
  (#at("reader.rs:29-146")) and the `ForeignExpander` seam that keeps `expand.rs` wasm-safe
  (#at("expand.rs:34-69")) are clean designs, both thoroughly tested.
- *Hygiene* — error messages are consistently actionable; the wasm32 gating in `lib.rs`
  keeps the native-only backend out of the playground build; the macro-component executor
  runs against an empty, capability-free linker (#at("host.rs:95-105")).

#v(0.6em)
#line(length: 100%, stroke: 0.4pt + luma(70%))
#text(size: 8pt, fill: luma(45%))[
  Findings verified against `target/debug/wavelet` (`read` / `repl` / `build`) and
  `wasm-tools print` on the emitted component. Full `cargo test` suite green at review time.
]
