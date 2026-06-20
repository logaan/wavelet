// compiler-pipeline.typ — how the Wavelet compiler works, as a diagram.
// Render: `typst compile dev-notes/compiler-pipeline.typ`
// Uses the `fletcher` package (auto-downloaded on first compile).

#import "@preview/fletcher:0.5.8" as fletcher: diagram, node, edge

#set page(paper: "a4", margin: (x: 1.4cm, y: 1.5cm))
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono")

// ---- palette ---------------------------------------------------------------
#let c-stage   = rgb("#eaf1fb"); #let s-stage  = rgb("#3b6db0") // pipeline stage
#let c-io      = rgb("#e9f6ec"); #let s-io     = rgb("#3f8f54") // source / artifact
#let c-side    = rgb("#fdf3e7"); #let s-side   = rgb("#b3792f") // feed-in
#let c-oracle  = rgb("#f3edfb"); #let s-oracle = rgb("#6b4fa3") // interpreter

#let sub(body) = text(font: "DejaVu Sans Mono", size: 6.5pt, fill: luma(38%), body)
#let stage(title, detail) = align(center, stack(spacing: 3pt, strong(title), sub(detail)))
#let elabel(body) = text(size: 7pt, fill: luma(30%), body)

#align(center, text(size: 15pt, weight: "bold")[How the Wavelet compiler works])
#align(center, text(size: 8.5pt, fill: luma(40%))[
  `wavelet build`: read → expand → analyze → emit → componentize → compose.
  The interpreter is a parallel path and the semantics oracle.
])
#v(2pt)

#align(center, diagram(
  spacing: (7mm, 9.5mm),
  node-corner-radius: 4pt,
  node-inset: 6pt,
  node-stroke: 0.9pt,
  label-sep: 2pt,

  // ---- main spine (the `build` path) ----
  node((0, 0), stage([`.wvl` source], [one file = one component]), name: <src>, fill: c-io, stroke: s-io),
  edge(<src>, <read>, "->"),
  node((0, 1), stage([read], [lexer · reader · form · printer]), name: <read>, fill: c-stage, stroke: s-stage),
  edge(<read>, <expand>, "->", elabel[form-tree arena · sugar resolved]),
  node((0, 2), stage([expand], [expand.rs — macros → fixpoint]), name: <expand>, fill: c-stage, stroke: s-stage),
  edge(<expand>, <wit>, "->"),
  node((0, 3), stage([analyze + WIT synth], [wit.rs — FileInfo · infer sigs · world]), name: <wit>, fill: c-stage, stroke: s-stage),
  edge(<wit>, <emit>, "->"),
  node((0, 4), stage([emit core wasm], [emit.rs — value boxes · tail calls · canonical ABI]), name: <emit>, fill: c-stage, stroke: s-stage),
  edge(<emit>, <comp>, "->", elabel[core module]),
  node((0, 5), stage([componentize], [wit-component — embed metadata + encode]), name: <comp>, fill: c-stage, stroke: s-stage),
  edge(<comp>, <unit>, "->"),
  node((0, 6), stage([per-file `.wasm`], [one component per source file]), name: <unit>, fill: c-io, stroke: s-io),
  edge(<unit>, <compose>, "->"),
  node((0, 7), stage([compose], [build.rs · wac — auto-plug sibling imports]), name: <compose>, fill: c-stage, stroke: s-stage),
  edge(<compose>, <app>, "->"),
  node((0, 8), stage([`app.wasm`], [linked artifact · runs on wasmtime]), name: <app>, fill: c-io, stroke: s-io),

  // ---- feed-ins (right) ----
  node((2.7, 2), stage([macro components], [macrodep · macros · host (wasmtime) \ macrobuild → wavelet:meta/macros]), name: <macro>, fill: c-side, stroke: s-side),
  edge(<macro>, <expand>, "->", elabel[foreign `macros: true`]),

  node((2.7, 3.7), stage([dependencies], [sibling `.wvl` (build set) \ wit/deps via witdep · wkg fetch]), name: <deps>, fill: c-side, stroke: s-side),
  edge(<deps>, <emit>, "->", elabel[sigs · types · WIT]),

  // ---- interpreter: parallel path + oracle (left) ----
  node((-2.9, 2.6), stage([interpreter], [interp · builtins · value]), name: <interp>, fill: c-oracle, stroke: s-oracle),
  edge(<expand>, <interp>, "->", elabel[expanded forms], label-side: left),
  node((-2.9, 5), stage([value / output], [run · repl · playground]), name: <out>, fill: c-oracle, stroke: s-oracle),
  edge(<interp>, <out>, "->"),
  edge(<interp>, <app>, "->", elabel[reference semantics — \ backend validated against], dash: "dashed", bend: -32deg),
))

#v(3pt)
#set text(size: 8pt)
#grid(columns: (auto, auto, auto, auto, auto, auto, auto, auto), column-gutter: 6pt, row-gutter: 4pt, align: horizon,
  box(width: 0.8em, height: 0.8em, radius: 2pt, fill: c-stage, stroke: s-stage), sub[compiler stage],
  box(width: 0.8em, height: 0.8em, radius: 2pt, fill: c-io, stroke: s-io), sub[source / artifact],
  box(width: 0.8em, height: 0.8em, radius: 2pt, fill: c-side, stroke: s-side), sub[feed-in],
  box(width: 0.8em, height: 0.8em, radius: 2pt, fill: c-oracle, stroke: s-oracle), sub[interpreter (oracle)],
)
#align(center, text(size: 7.5pt, fill: luma(40%))[
  Solid = data flow · dashed = "the wasm backend must agree with the interpreter" (`CLAUDE.md`). \
  `wavelet run` / `repl` / the docs playground stop at the interpreter; `build` follows the full spine.
])
