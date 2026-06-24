#set document(title: "Wavelet: exhaustive language feature inventory")
#set page(paper: "a4", margin: (x: 2cm, y: 2cm), numbering: "1")
#set text(font: "New Computer Modern", size: 10pt)
#set par(leading: 0.55em)
#show heading: set block(above: 1.0em, below: 0.5em)
#set heading(numbering: "1.1  ")
#show raw.where(block: false): box.with(
  fill: luma(240), inset: (x: 3pt, y: 0pt), outset: (y: 3pt), radius: 2pt,
)
#set list(indent: 0.6em, spacing: 0.45em)

#align(center)[
  #text(16pt, weight: "bold")[Wavelet: exhaustive language feature inventory]
  #v(0.2em)
  #text(9pt, style: "italic")[
    A flat naming of every language construct, type, form, operation, and rule.
    Language only — no tooling, CLI, editor, or build-pipeline surface.
  ]
]

#v(0.4em)

This is a complete checklist of the *language*: data, forms, evaluation,
macros, types, and the component/WIT-level constructs that are part of its
semantics. It names features; it does not explain them. An item appears whether
or not it is finished or correct.

= Lexical tokens

== Whitespace & separators
- Space, tab, carriage return, newline
- Comma (whitespace; `[1, 2]` ≡ `[1 2]`)
- Line comment `//` to end of line

== Literal tokens
- Boolean `true`, `false`
- Integer (`i64` lexeme; e.g. `123`, `-9`)
- Decimal / float (`f64` lexeme; e.g. `3.14`, `6.022e+23`, `1e-3`)
- `inf`, `nan`, `-inf` (whole-word float literals)
- Char `'x'`, `'☃'`, `'\u{0}'`
- String `"..."`

== String / char escapes
- `\n`, `\t`, `\r`, `\\`, `\"`, `\'`
- `\u{...}` Unicode scalar escape

== Identifier tokens
- Kebab-case identifier (WIT label: hyphen-separated words, each all-lowercase or all-UPPERCASE)
- `%` escape for reserved/keyword identifiers (`%ok`)
- Reserved WAVE keywords: `true`, `false`, `inf`, `nan`, `some`, `none`, `ok`, `err`
- TitleCase macro-head token (kebab-lowered, `-MACRO` suffix)
- Qualified name `alias/name`
- Qualified TitleCase macro head `Alias/Name`

== Punctuation tokens
- `(` `)` `[` `]` `{` `}`
- `:` (record field separator)
- `.` (call-chaining dot; decimal point only when a digit follows)

= Reader sugar & surface forms

== Attachment rule
- Only a `(` abutting an identifier (no whitespace) forms a call
- Attached `[` / `{` is a read error (list/record call sugar removed)

== Call forms
- `f(x)` one argument
- `f(x y)` positional arguments
- `f()` zero-argument call
- `f([x y])` list argument
- `f({a: 1 b: 2})` record argument (named arguments)
- Qualified call `kv/get({...})`
- Free-standing `(a b)` → tuple/call node; `(a)` zero-arg call; `()` empty tuple

== Call chaining
- `recv.name(args)` → `(name, recv, …args)`
- Left-to-right fold of chains
- Whitespace-sensitive (`.`, name, `(` each abut)

== Macro sugar
- TitleCase head → arity-driven paren-free reading
- Explicit-payload form `If(c t e)` ≡ `If c t e`
- Fully explicit `(if-MACRO, c, t, e)`
- Nesting without delimiters (last argument may be a macro form)
- Variadic macro via single list argument (`Do [a b c]`)
- Define-before-use ordering requirement

== Macro arity table & resolution
- Core special-form arities
- File-local `DefMacro` registration (top-to-bottom)
- Foreign macro arities from `macros: true` imports
- Bare-name resolution
- Qualified `Alias/Name` resolution
- Ambiguity detection (bare collision → error; qualified still resolves)
- Origin tracking (local vs per-alias import)

== Literal data forms
- List `[a b]`
- Record `{k: v}`
- Flags `{read write}`, empty `{}`
- Derive-class flags `{Eq Ord Show Hash}` (TitleCase flag entries, Derive's first argument only)

= Values (the data model is WIT's)

== Runtime value kinds
- Bool
- Int (integer)
- Dec (decimal/float)
- Char
- Str (string)
- Tup (tuple)
- Lst (list)
- Rec (record)
- Flg (flags)
- Variant (case label + optional payload)
- Symbol (payload-less variant case)
- Closure (first-class function)
- Macro (compile-time function value)
- Builtin
- Cell (mutable reference)
- Unit (empty record `{}`)

== WIT type inventory (literal surface)
- `bool`
- `u8`, `u16`, `u32`, `u64`
- `s8`, `s16`, `s32`, `s64`
- `f32`, `f64`
- `char`
- `string`
- `tuple<…>` (written via `Quote`)
- `list<t>`
- `record`
- `variant`
- `enum`
- `option<t>` (`some(x)`, `none`, flat)
- `result<t,e>` (`ok(x)`, `err(e)`, flat)
- `flags`
- `resource` (handles)
- `func` type form (callback parameters)

== Value rules
- Integer literals default `s64`; range-checked at narrower targets
- Float literals default `f64`
- `option` / `result` flat shorthand at typed boundaries
- `some` / `none` / `ok` / `err` constructors
- Structural equality for data; identity equality for resource handles
- Canonical WAVE printing (strict, comma-separated)

= Core forms

== The seventeen
- `Package` — package id + version
- `Import` — import an interface
- `Export` — export a definition or type
- `DefType` — declare a WIT type
- `Def` — immutable module-level binding
- `Fn` — closure
- `If` — conditional
- `Let` — sequential local bindings (`let*`)
- `Do` — sequencing; value of last
- `Match` — pattern matching
- `Quote` — form as data
- `Quasi` — template
- `Unquote` — evaluate-and-insert hole
- `Splice` — evaluate-to-list-and-splice hole
- `DefMacro` — compile-time form→form function
- `The` — type ascription
- `Derive` — derive standard operations for a type

= Evaluation

== Model
- Lisp-1 (single namespace)
- Atoms self-evaluate
- Name → lexical-environment lookup
- Qualified name lookup
- Call: evaluate args, bundle (0 → empty tuple, 1 → value, ≥2 → tuple), apply head
- Functions take exactly one value
- `apply` for computed function values

== Parameter binding
- Record argument binds by name
- List/tuple argument binds by order
- Scalar argument binds sole parameter
- Untyped parameters (`Fn {a b}`)
- Typed parameters (`Fn {a: string b: bool}`)
- Per-width integer range check on typed parameters
- Zero-parameter functions (`Fn {}`)

== Environments
- Root / child environment chaining
- Lexical capture in closures

== Tail-call elimination
- Guaranteed within a component
- Tail positions: `Fn` body, both `If` branches, every `Match` result, last `Do` expression, `Let` body
- Constant-stack tail / mutual recursion

== Pattern matching (`Match`)
- Literal patterns (bool, int, dec, char, str, flags) by equality
- Wildcard / binding name
- Enum-case name by equality (when bound to a payload-less variant)
- Variant-case destructure `(case …rest)`
- Tuple destructure (element-wise)
- List destructure
- Record destructure (by field)
- Tuple/variant disambiguation by scrutinee
- Multi-field variant payload destructure
- No-clause-matched error

== Type ascription (`The`)
- Pin a literal's type
- Runtime conformance check
- Drive numeric resolution / overload return-type selection

= Macros

== Definition & templating
- `DefMacro` (forms → form)
- `Quote`
- `Quasi` (with nesting depth)
- `Unquote` (depth-aware)
- `Splice` (into list/tuple/call)
- `gensym` (fresh names)
- Unhygienic expansion (gensym discipline)

== Expansion
- Ahead-of-time expansion to fixpoint
- Lazy expansion at eval time (interpreter)
- One-step expansion (`expand`)
- `DefMacro` forms evaluated then dropped from output

== Macro components
- Foreign macros via `Import {pkg: "…" macros: true}`
- `wavelet:meta/macros` contract: `manifest()` → `(name, arity)` pairs, `expand(name, args)` → `result<tree, string>`
- Compile-time instantiation of macro libraries
- Macro libraries authored in Wavelet (`Package` + `DefMacro`s only)
- Macro bodies compiled to wasm
- Interpreter as differential oracle
- Compile-time sandbox (no host capabilities)
- Bare and qualified (`alias/Name`) foreign-macro use
- Ambiguity errors only on bare use

== `Derive`
- `Eq` → `eq-{t}`
- `Ord` → `compare-{t}`
- `Show` → `show-{t}`
- `Hash` → `hash-{t}`
- Expands to monomorphic `Def` + auto-`Export` per class

= Type system

== Discipline
- Monomorphic
- Total static checking (every definition, even unused)
- Bidirectional inference (inward propagation + outward synthesis)
- Gradual top (`Unknown`) for unmodelled constructs
- Dynamically typed core / statically typed edges

== Type lattice
- `Bool`
- `U8`, `U16`, `U32`, `U64`
- `S8`, `S16`, `S32`, `S64`
- `F32`, `F64`
- `Char`
- `String`
- `List<T>`
- `Named` (nominal `DefType`)
- `Unit`
- `IntLit` (unresolved integer literal)
- `FloatLit` (unresolved float literal)
- `Unknown`

== Numeric literals
- Context-driven resolution
- Range check against target width
- Default `IntLit` → `s64`, `FloatLit` → `f64`
- Mixed int/float literal → float literal
- Shared integer-bounds source of truth (`int_fits`)

== Unification
- `Unknown` absorbs anything
- Integer literal unifies with any concrete int/float
- Float literal unifies with concrete floats only
- Element-wise list unification

== Overloading
- Overload sets (≥2 same-named monomorphic `Def`s)
- Resolution by argument WIT types
- Resolution by expected return type (via `The`)
- Curated overloadable ops (`eq`, `compare`, `show`, `hash`, comparisons, arithmetic)
- Name-mangling at the component boundary (`eq-point`)
- First-parameter mangling, full-parameter disambiguation on collision
- Duplicate-member compile error

= Code as data (homoiconic meta layer)

== Form nodes
- `Bool`, `Int`, `Dec`, `Char`, `Str`
- `Sym` (symbol)
- `Qsym` (qualified symbol)
- `Tup` (call = tup whose head is element 0)
- `Lst`
- `Rec`
- `Flg`

== Wire encoding (`wavelet:meta/code`)
- `node-id`
- `node` variant (`bool-val`, `int-val`, `dec-val`, `char-val`, `str-val`, `sym`, `qsym`, `tup`, `lst`, `rec`, `flg`)
- `tree` record (`nodes`, `root`, `spans`)
- Arena ↔ tree conversion
- Source spans parallel to nodes

== Form operations
- `to-string` (form → canonical WAVE)
- `read` (WAVE string → form)
- Lossless sugar-in / canonical-out round trip
- `form-kind`
- `rec-key`
- `rec-val`

= Standard library (`wavelet:std/core`)

== Predicates
- `eq`, `lt`, `le`, `gt`, `ge`, `not`

== Arithmetic
- `add`, `sub`, `mul`, `div`, `rem`, `neg`, `min`, `max`, `abs`

== Sequences
- `len`, `empty`, `get`, `put`, `push`, `concat`, `head`, `tail`, `reverse`, `range`, `map`, `filter`, `fold`, `zip`

== Strings
- `str-cat`, `upper`, `lower`, `split`, `join`, `contains`

== Conversion
- `to-string`, `read`
- `to-u8`, `to-u16`, `to-u32`, `to-u64`
- `to-s8`, `to-s16`, `to-s32`, `to-s64`
- `to-f32`, `to-f64`

== Meta
- `apply`, `gensym`, `expand`
- `form-kind`, `rec-key`, `rec-val`

== Constructors & values
- `some`, `ok`, `err`
- `none` (bound value)
- `pi` (bound value)

== Resources & mutable state
- `cell-new`, `cell-get`, `cell-set`
- `drop`

== Library macros (user-defined; not shipped)
- `And` (short-circuit)
- `Or` (short-circuit)
- `TryLet` (error propagation / binding form)

= Components, interfaces & WIT-level constructs

== File / component model
- One file = one component
- File anatomy: `Package`, `Import` (any number), `DefType`, `Def`, `Export`
- WIT world synthesis from typed `Fn` params, inference, and `Export` records
- Default export interface `api`
- Grouped exports (`Export {iface: "…" …}`)
- Explicit export signature (`Export {name: … params: … result: …}`)

== Imports
- Bare path `Import "wasi:cli/environment"` (alias = last segment)
- `Import {pkg: "…" as: alias}`
- `Import {pkg: "…" open: true}` (splat unqualified)
- `Import {pkg: "…" macros: true}` (compile-time macro library)
- `Import {pkg: "…" elem: t as: …}` (functor instantiation)
- `Import {pkg: "…" from: path}` (macro-library `.wasm` location)
- Implicit `Import {pkg: "wavelet:std/core" open: true}` (disable via `std: false`)
- Qualified imported names (`kv/open`, `kv/get`)

== Types & functors
- `DefType` → WIT type declaration
- Nominal records / variants / enums / flags
- Functor instantiation → specialized monomorphic interface
- `Set` functor operations: `new`, `add`, `contains`, `size`
- Specialized interface naming (`point-set`, `point-set.set`)

== Resources
- Resource handles
- Methods as functions (first parameter is the handle)
- Constructor-returned handles
- Owned-handle scope drop / explicit `drop`
- `own<T>` / `borrow<T>`

== Closures across boundaries
- Escaping closure lifted to single-method resource
- `call` method convention
- Inbound resources with a lone `call` invoked as ordinary calls
- `func` type form for declared callbacks

== Boundary semantics
- Identical call syntax for local and imported functions
- Canonical-ABI copy at component boundaries
- Tail calls bounded by the component boundary
- Cross-component tail calls restored under fusion (`--fuse`)
- `host` passthrough imports
- Macro imports excluded from the runtime world

= Backend semantics (interpreter / wasm parity)

- Interpreter as the semantics oracle
- Canonical-ABI lift/lower for scalars, lists, records, variants, tuples, `option`, `result`, closures across boundaries
- `return_call` / `return_call_indirect` tail calls
- Checked integer overflow; strictly binary arithmetic
- Runtime float/string dispatch for arithmetic and comparison
- Resource handles carried as i32
- GC-types backend with linear-memory fallback
