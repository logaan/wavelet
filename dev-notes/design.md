# Wavelet

**A language design — draft 0.1**

Wavelet is a small homoiconic language for the WebAssembly Component Model. It rests on three commitments.

First, *one file is one component*. The unit you edit, the unit you compile, and the unit you link, version, and deploy are the same thing. A Wavelet program is a composition of components, and nothing distinguishes a component written in Wavelet from one written in Rust, Go, or JavaScript — composition happens at the WIT level.

Second, *the syntax is WAVE*. [WAVE](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-wave) (WebAssembly Value Encoding) is the human-readable text encoding for Component Model values. Wavelet source code is WAVE text plus a thin layer of reader sugar, and every sugared form desugars to a plain WAVE value. The language is homoiconic the way Lisp is homoiconic over s-expressions — except Wavelet's "s-expressions" are exactly the values that cross component boundaries. Code is data, and the data is the Component Model's data.

Third, *the core is minimal*. Seventeen special forms, closures, guaranteed tail-call elimination, and a macro system. Everything else — including the standard library, and including macros — is delivered as components.

These three commitments reinforce each other in one unusual way that deserves stating up front: **there is no FFI**. Wavelet has no native data types of its own. Its booleans, strings, lists, records, variants, options, results, and flags *are* WIT types. Calling a Rust component looks identical to calling the function defined two lines up, because there is no representation gap to bridge.

---

## 1. A taste

```
// shout.wvl — compiles to demo:shout.wasm
Package "demo:shout@0.1.0"

Export shout
Def shout Fn {phrase: string}
  str-cat(upper(phrase) "!")
```

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

```console
$ wavelet build *.wvl
$ wavelet compose out/*.wasm -o app.wasm
$ wasmtime app.wasm wasm
WASM!
```

Each file declared its own package, became its own component, and the composer wired `main`'s import of `demo:shout/api` to `shout`'s export. Swapping in a Rust implementation of `demo:shout/api` would require changing nothing in `main.wvl`.

---

## 2. Reading Wavelet

### 2.1 Lexical ground rules

Wavelet's tokens are WAVE's tokens. Identifiers are kebab-case labels following WIT identifier syntax: hyphen-separated words, each word either all-lowercase or all-UPPERCASE (`delete-file`, `parse-JSON`). The WAVE keywords `true false inf nan some none ok err` are reserved; an identifier that collides with one is written with WIT's `%` escape (`%ok`). Comments are `//` to end of line, as in WAVE and WIT.

**Commas are whitespace.** As in Clojure, `[1, 2, 3]` and `[1 2 3]` read identically. Wavelet source is therefore a superset of WAVE text: any valid WAVE value is a valid Wavelet form. The canonical printer always emits strict WAVE (with commas), so printed code can be consumed by any existing WAVE tooling.

Newlines are also just whitespace. Wavelet's only whitespace-sensitive rules are about *attachment* — a token abutting the one before it with no space between: the call attachment rule (§2.2) and call chaining (§2.5).

### 2.2 The attachment rule

Only a `(` that *immediately* follows an identifier, with no intervening whitespace, forms a call: the identifier becomes the call's head and the parenthesized forms its arguments. A `[` or `{` no longer attaches — the old list/record call sugar was removed, and attaching either is a read error pointing at the new spelling (`name([…])` / `name({…})`). With whitespace in between, the bracketed expression is a separate, free-standing value.

```
delete-file({path: "foo.md" force: true})  // a call: the head plus a record argument
delete-file{path: "foo.md" force: true}    // read error — `{` no longer attaches
delete-file {path: "foo.md" force: true}   // two forms: a variable, then a record
```

### 2.3 Desugaring

Every surface form desugars to a canonical WAVE value. The complete table:

| You write | Canonical WAVE form | Meaning |
|---|---|---|
| `true` `42` `-1.5` `'x'` `"hi"` | (unchanged) | atom, self-evaluating |
| `foo` | `foo` | variable reference — a bare, payload-less case (a symbol) |
| `f(x)` | `(f, x)` | call with one argument |
| `f(x y)` | `(f, x, y)` | call with positional arguments |
| `f([x y])` | `(f, [x, y])` | call with a list argument |
| `f({a: 1 b: 2})` | `(f, {a: 1, b: 2})` | call with a record argument — named arguments |
| `f()` | `(f)` | zero-argument call |
| `kv/get({...})` | qualified call | name `get` from the import aliased `kv` |
| `x.f(y)` | `(f, x, y)` | call chaining — receiver is the first argument (§2.5) |
| `(a b)` | `(a, b)` | a parenthesized form — a call in evaluation position, a tuple under `Quote` |
| `(a)` | `(a)` | a zero-argument call of `a` (no transparent grouping) |
| `()` | `()` | the empty tuple (an error if evaluated) |
| `[a b]` | `[a, b]` | list |
| `{k: v}` | `{k: v}` | record |
| `{read write}` | `{read, write}` | flags |
| `If c t e` | `(if-MACRO, c, t, e)` | macro use (see §2.4) |
| `Unquote(x)` | `(unquote-MACRO, x)` | macro use with explicit payload |

Function calls are tuples whose first element is the head. `print("hi")` is, as data, the WAVE tuple `(print, "hi")`; `delete-file({path: "foo.md" force: true})` is the tuple `(delete-file, {…})`. A bare identifier is a payload-less variant case (a symbol), and Wavelet reads it as a variable reference. In evaluation position a parenthesized form is *always* a call — evaluating its head and applying it to the bundled arguments (§4); a literal tuple **value** is obtained only via `Quote`, a builtin, or pattern binding. The list/record call sugar (`f[…]`, `f{…}`) was removed; write `f([…])` and `f({…})`.

### 2.4 The macro sugar

A TitleCase identifier — mixed-case words, each starting with a capital (`If`, `DefMacro`, `TryLet`) — is reader sugar for a macro call. Note this cannot collide with ordinary identifiers: WIT words are all-lower or all-UPPER, so TitleCase tokens are syntactically free real estate. The token kebab-izes and gains a `-MACRO` suffix (an all-caps word, which *is* a legal WIT identifier): `If` ↦ `if-MACRO`, `TryLet` ↦ `try-let-MACRO`.

A TitleCase head does not require parentheses around its arguments. Instead, the reader looks up the macro's declared **arity** and consumes exactly that many following forms, splicing them after the head into the call tuple:

```
If eq(foo bar) print("match") print("nope")
```

desugars to

```
(if-MACRO, (eq, foo, bar), (print, "match"), (print, "nope"))
```

Three consequences of arity-driven reading:

A macro must be **in scope before use** — defined earlier in the file, a core form, or imported from a component that publishes a macro manifest (§6.3). The reader processes a file top to bottom and always knows every visible macro's arity.

An **explicit payload reads identically to the arity form**: `Unquote(x)` and `If(c t e)` attach their arguments directly, which is occasionally clearer inside dense templates — `If(c t e)` and `If c t e` both read to `(if-MACRO, c, t, e)`. The fully explicit spelling `(if-MACRO, c, t, e)` is also always available, and is what macros emit when generating code.

**Nesting needs no delimiters** when the last argument is itself a macro form: `Def run Fn {} If c a b` reads exactly as intended, since each TitleCase head consumes its own arity's worth of forms, recursively. Sibling macro forms inside a list or argument are naturally bounded by the enclosing bracket; when two macro forms must sit side by side with nothing enclosing them, wrap one in a call (`(…)`).

Variadic macros take a single list argument: arity stays fixed, the list flexes (`Do [a b c]`).

### 2.5 Call chaining

A `.` immediately following a form, then a name and an attached `(`, is **call chaining**. The receiver is folded in as the call's *first* argument: `recv.name(args)` reads as `(name, recv, …args)`. Chains fold left-to-right, so each `.name(…)` wraps everything to its left:

```
1.increment()                       // (increment, 1)
foo(1 2 3).bar(4 5 6).baz(7 8 9)    // (baz, (bar, (foo, 1, 2, 3), 4, 5, 6), 7, 8, 9)
```

Like the attachment rule (§2.2) this is whitespace-sensitive: the `.`, the name, and the `(` must each abut the token before them. The reader achieves this by lexing `.` as a decimal point only when a digit follows, so `1.increment` lexes as `1` then `.` then `increment`.

This is **pure reader rewriting, not method dispatch** — nothing is attached to the receiver, and `1.increment()` is exactly `increment(1)`. The receiver and the parenthesized arguments are arbitrary forms, so the call name can be any function (`add`, an imported `kv/get`, …); the only purpose is letting a pipeline read left-to-right instead of inside-out.

---

## 3. Values: the Component Model is the data model

Wavelet has no value types beyond WIT's. The full inventory, with WAVE literals:

| WIT type | Wavelet/WAVE literal | Notes |
|---|---|---|
| `bool` | `true`, `false` | |
| `u8…u64`, `s8…s64` | `123`, `-9` | integer literals are `s64` by default; coerced with a range check where a narrower type is expected |
| `f32`, `f64` | `3.14`, `6.022e+23`, `nan`, `-inf` | float literals are `f64` by default |
| `char` | `'x'`, `'☃'`, `'\u{0}'` | Unicode scalar values |
| `string` | `"abc\t123"` | |
| `tuple<…>` | `Quote ("abc", 123)` | a parenthesized form is a *call* in evaluation position, so a literal tuple value is written with `Quote` (or produced by a builtin) |
| `list<t>` | `[1 2 3]` | |
| `record` | `{field-a: 1 field-b: "two"}` | |
| `variant` | `days(30)`, `forever` | case label, parenthesized payload if any |
| `enum` | `south`, `west` | cases are bound as ordinary names on import |
| `option<t>` | `some(1)`, `none`, or flat `1` | WAVE's flat shorthand applies at typed boundaries |
| `result<t,e>` | `ok(1)`, `err("oops")`, or flat `1` | likewise |
| `flags` | `{read write}`, `{}` | |
| `resource` | handles | created/consumed via imported constructors and methods |

**Wavelet is dynamically typed in its core and statically typed at its edges.** Inside a component, values carry their shape at runtime, lists may be heterogeneous, and records are structural. At every component boundary there is a WIT signature, and the compiler checks what it can statically (all literals, all inferable flows) and inserts checked coercions for the rest; a dynamic value that fails to conform to the boundary type traps, or — under the `safely` wrapper from the standard library — returns an `err`. Annotations tighten things ahead of the boundary: typed `Fn` parameters (§4.2) and the `The` ascription form push checking to compile time.

`some`, `none`, `ok`, and `err` are ordinary constructors bound in the core library; the flat shorthands engage wherever a WIT `option`/`result` type is expected, exactly as WAVE specifies.

Equality (`eq`) is structural for all data types and identity-based for resource handles.

---

## 4. Evaluation

### 4.1 The rules

There are four:

1. **Atoms** — booleans, numbers, chars, strings, flags — evaluate to themselves.
2. **A bare name** evaluates to its binding in the current lexical environment. Wavelet is a Lisp-1: one namespace for everything. Unbound names are compile-time errors. Enum cases, imported functions, and constructors are all just bindings.
3. **A call form** — a tuple `(head arg…)` in evaluation position — evaluates its arguments, **bundles** them (0 ⇒ the empty tuple, 1 ⇒ that value, ≥2 ⇒ a tuple), and applies the value bound to `head`. Heads are names or qualified names; evaluating a tuple whose head is not a name is an error, and a literal tuple value comes from `Quote` or a builtin. To call a computed function value, use `apply(f payload)`.
4. **Special forms and macros** are recognized by the expander before evaluation and follow their own rules.

### 4.2 The seventeen special forms

This table is the entire core language.

| Form | Arity | Shape of arguments | Meaning |
|---|---|---|---|
| `Package` | 1 | string | declare this component's package id and version |
| `Target` | 1 | string | adopt a named WIT world, e.g. `"wasi:cli/command"` |
| `Import` | 1 | string or record | import an interface (§6.1) |
| `Export` | 1 | name or record | export a definition or type (§6.1) |
| `DefType` | 2 | name, type form | declare a WIT type for this component's interface |
| `Def` | 2 | name, expression | immutable module-level binding |
| `Fn` | 2 | parameter braces, body | closure |
| `If` | 3 | condition, then, else | conditional |
| `Let` | 2 | binding record, body | sequential local bindings |
| `Do` | 1 | list of expressions | sequencing; value of the last |
| `Match` | 2 | scrutinee, clause list | pattern matching |
| `Quote` | 1 | form | the form itself, as data |
| `Quasi` | 1 | form | template with holes |
| `Unquote` | 1 | form | hole: evaluate and insert |
| `Splice` | 1 | form | hole: evaluate to a list and splice in |
| `DefMacro` | 3 | name, parameter braces, body | compile-time function from forms to a form |
| `The` | 2 | type form, expression | type ascription |

**Functions take exactly one value.** That value may be a record, list, tuple, or scalar, which is what gives calls their n-ary feel. `Fn`'s parameter braces describe how to receive it:

```
Fn {path force}              // two parameters, dynamically typed
Fn {path: string force: bool}  // two parameters, typed (this is a record form;
                               // a name-only brace form is WAVE flags syntax —
                               // both are just data the Fn form interprets)
Fn {phrase}                  // one parameter
Fn {}                        // zero parameters
```

At a call site, a record argument binds parameters **by name**, a list or tuple argument binds them **by order**, and a scalar argument binds a sole parameter directly. So both of these reach the same function:

```
delete-file({path: "foo.md" force: true})
delete-file("foo.md" true)
```

**`Let`** takes its bindings as a record form — homoiconicity doing the work of syntax — and binds sequentially, like `let*`:

```
Let {radius: 10
     area: mul(pi mul(radius radius))}
  str-cat("area = " to-string(area))
```

**`Match`** takes a scrutinee and a list of `(pattern result)` tuples. Patterns are just forms: literals match by equality, a bare name matches anything and binds it, a `(case …)` form whose head is a name destructures a variant case, and lists, tuples, and records destructure their counterparts (a tuple pattern is disambiguated against the scrutinee — a variant value takes the variant-case reading, a tuple value the element-wise one).

```
Match read-file({path: "notes.md"}) [
  (ok(text)              process(text))
  (err(not-found)        println("no such file"))
  (err(e)                println(str-cat("error: " to-string(e))))
]
```

**Closures** capture lexically and are first-class within a component. (Crossing a boundary with one is handled in §6.4 — the Component Model has no function values, so the toolchain lifts escaping closures into resources.)

There is no loop construct, no mutation form, and no early return. Iteration is tail recursion (§5); mutable state lives in resources (a `cell` resource ships in the standard library); error propagation is a macro over `Match` (§7.2).

---

## 5. Tail calls

Wavelet guarantees tail-call elimination. Tail positions are: the body of a `Fn`; both branches of an `If`; every clause result of a `Match`; the last expression of a `Do`; and the body of a `Let`. A call in tail position compiles to the wasm `return_call` / `return_call_indirect` instructions from the (now widely shipped) tail-call proposal, so tail recursion runs in constant stack regardless of mutual-recursion structure:

```
Def count-down Fn {n}
  If eq(n 0)
     "liftoff"
     count-down(sub(n 1))     // constant stack, any n
```

One honest caveat: the guarantee holds **within a component**. A call to an imported function passes through canonical-ABI adapters that the composer generates, and those frames are outside Wavelet's control; cross-component cycles therefore consume stack. The compiler warns when it can see an unbounded recursion routed through an import. (The `--fuse` optimization in §6.5 dissolves this boundary for all-Wavelet subgraphs.)

---

## 6. Files, components, composition

### 6.1 Anatomy of a file

A file is a flat sequence of forms: an optional `Package`, optional `Target`, any number of `Import`s, then `DefType`s, `Def`s, and `Export`s in any order (definitions must precede macro *use*, but value references resolve file-wide). The compiler synthesizes a WIT world from these. For `shout.wvl` in §1 it produces:

```wit
package demo:shout@0.1.0;

interface api {
  shout: func(phrase: string) -> string;
}

world shout {
  export api;
}
```

Exported functions get their WIT signatures from typed `Fn` parameters, from inference against use, or from an explicit record form: `Export {name: shout params: {phrase: string} result: string}`. A file's exports land in a default interface named `api`; `Export {iface: "render" ...}` groups them otherwise.

`Import` takes a package path string, optionally a record for control:

```
Import "wasi:cli/environment"                       // alias defaults to last segment
Import {pkg: "wasi:keyvalue/store@0.2.0" as: kv}    // explicit alias
Import {pkg: "acme:html/dsl" macros: true}          // load macro manifest too (§6.3)
Import {pkg: "demo:text/api" open: true}            // splat names in unqualified
```

Imported names are used qualified, Clojure-style: `kv/open`, `kv/get`. Every file implicitly does `Import {pkg: "wavelet:std/core" open: true}` (disable with `Package {id: "..." std: false}`).

Resources come along for free, because WIT canonicalizes methods as functions whose first parameter is the handle. `kv/open({name: "default"})` returns a handle; `kv/get(bucket "greeting")` calls a method. Owned handles are dropped when their binding scope ends without the handle escaping; `drop(h)` forces it.

### 6.2 Code as a WIT type

Homoiconicity has to survive the boundary, so "a form" is itself a WIT type, defined in `wavelet:meta`. Logically a form is recursive:

```
form = bool | int | dec | char | str
     | sym(name) | qsym(alias name)
     | tup(forms) | lst(forms) | rec(fields) | flg(names)
```

A call is just a `tup` whose first element is the head, so there is no separate `call` node.

WIT has no recursive types, so the wire encoding is an arena — a flat node table plus a root index:

```wit
package wavelet:meta@0.1.0;

interface code {
  type node-id = u32;

  variant node {
    bool-val(bool),
    int-val(s64),
    dec-val(f64),
    char-val(char),
    str-val(string),
    sym(string),
    qsym(tuple<string, string>),
    tup(list<node-id>),                // a call is a tup whose head is items[0]
    lst(list<node-id>),
    rec(list<tuple<string, node-id>>),
    flg(list<string>),
  }

  record tree {
    nodes: list<node>,
    root: node-id,
    spans: list<tuple<u32, u32>>,      // source offsets, parallel to nodes
  }
}
```

Inside Wavelet you never see the arena: `Quote` hands you a natural tree, and the runtime keeps forms as ordinary structured values. The arena is purely the interchange format — the moment a form crosses a boundary, it is a `tree`. `to-string(form)` prints canonical WAVE text; `read(string)` parses it; the round trip is lossless (sugar reads in, canonical prints out).

### 6.3 Macros are components

`DefMacro` defines a compile-time function from argument forms to a result form:

```
DefMacro and {a b}
  Quasi If Unquote(a) Unquote(b) false

And lt(x 10) gt(x 0)      // ⇒ If lt(x 10) gt(x 0) false
```

`Quasi` builds templates; `Unquote` evaluates a hole; `Splice` evaluates a hole to a list and splices its elements into the surrounding list, tuple, or call; `gensym()` mints fresh names. Expansion is unhygienic in the Common Lisp/Clojure tradition — `gensym` is the discipline, hygiene is future work (§10).

Because macros are functions over a WIT type, they need not be written in Wavelet. A component exporting `wavelet:meta/macros` is a macro library:

```wit
interface macros {
  use code.{tree};
  manifest: func() -> list<tuple<string, u32>>;          // (name, arity) pairs
  expand: func(name: string, args: tree) -> result<tree, string>;
}
```

`Import {pkg: "acme:html/dsl" macros: true}` instantiates that component **at compile time**, registers its manifest with the reader (this is how TitleCase arities are known across components), and routes expansion through `expand`. Two things fall out. You can write macros in Rust — say, a `Re` macro that compiles a regex literal to a DFA at build time. And macro expansion is **sandboxed by construction**: an untrusted macro runs inside a wasm component with no capabilities beyond what the build grants it. Compile-time code execution without the supply-chain terror.

Within a single namespace, macro name collisions are errors, resolved by aliasing the import; a qualified TitleCase form `Dsl/Element` disambiguates at use sites.

### 6.4 Closures across boundaries

WIT has no first-class functions, so a closure that escapes its component is automatically lifted to a single-method resource, and the signature appears in the synthesized WIT:

```wit
resource fn-string-to-string {
  call: func(arg: string) -> string;
}
each: func(f: borrow<fn-string-to-string>, items: list<string>) -> list<string>;
```

Inbound, the same duck-typing applies: any imported resource exposing a lone `call` method is invocable with ordinary call syntax. A Rust component can hand Wavelet a "closure" and vice versa; neither side writes glue. Declaring a callback parameter on an export uses the `func` type form: `{visit: func({params: {item: string} result: bool})}`.

### 6.5 Composition

`wavelet build` turns files into components; `wavelet compose` links them. By default the composer auto-plugs: every unresolved import is matched by package id (and version compatibility) against the exports of the components on the command line, in the spirit of `wac plug`. Ambiguities or substitutions are settled in a manifest that is — naturally — a WAVE document:

```
// compose.wave
{app: "out/main.wasm"
 plug: [{import: "demo:shout/api"   with: "out/shout.wasm"}
        {import: "acme:words/picker" with: "vendor/words-rs.wasm"}
        {import: "wasi:cli/environment" with: host}]}
```

Anything left to `host` flows out of the composed component for the runtime to satisfy. Under the hood this drives standard `wasm-tools compose` / WAC machinery, so Wavelet components participate in any component ecosystem workflow.

Boundary calls copy data (canonical ABI). For graphs that are entirely Wavelet, `wavelet compose --fuse` merges the core modules into one, eliding copies and restoring cross-component tail calls, with identical observable semantics. Componenthood is a deployment boundary you can dissolve when you own both sides.

---

## 7. Frictionless interop, itemized

The headline claim — calling other components is frictionless — cashes out as a stack of small decisions already described, gathered here. Call syntax is identical for local and imported functions, so `kv/set({bucket: b key: "k" value: bytes})` reads like any other call. There is no marshalling layer to think about because Wavelet values *are* canonical-ABI values; what you pass is what arrives. WAVE's flat `option`/`result` shorthands mean you write `"hello"` where `option<string>` is expected. Integer literals adapt to the target width with compile-time range checks. One `Import` line brings in an interface; `wavelet add wasi:keyvalue` fetches WIT from a registry, pins a version, and feeds editor completion. The toolchain consumes interfaces from `.wit` text or directly from a compiled `.wasm` binary, so "a library" is anything componentized, from any language. Exporting in the other direction is the single word `Export`. And macros imported from foreign-language components behave exactly like native ones.

### 7.1 The standard library, briefly

`wavelet:std/core` is itself a component (implicitly imported, open). Because identifiers are WIT labels, there is no `+` or `<` — a deliberate cost of "the syntax is WAVE, no exceptions." The names are short and boring: predicates `eq lt le gt ge not`; arithmetic `add sub mul div rem neg min max abs`; sequences `len empty get put push concat head tail reverse range map filter fold zip`; strings `str-cat upper lower split join contains`; conversion `to-string read to-u8 … to-f64`; I/O wrappers over WASI `print println read-line args env`; meta `apply gensym expand` plus form accessors `form-kind rec-key rec-val` and kin; resources `drop` and the mutable `cell`.

### 7.2 Library macros

The core's seventeen forms deliberately omit conveniences that macros supply. `And`/`Or` (short-circuit, arity 2, chain by nesting) appeared in §6.3. Error propagation has no early return to lean on, so it is a *binding* form — `TryLet` (arity 2) wraps the rest of the computation rather than escaping it:

```
DefMacro try-let {binding body}        // binding is a one-field record form
  Let {name: rec-key(binding) expr: rec-val(binding)}
    Quasi Match Unquote(expr) [
      (ok(Unquote(name))  Unquote(body))
      (err(e)             err(e))
    ]
```

```
Def load-config Fn {path: string}
  TryLet {text: read-file({path: path})}
  TryLet {form: read(text)}
  ok(form)
```

Rust's `?`, reconstructed in user space from `Match` and quasiquote.

---

## 8. A worked example, three languages deep

A tiny program: pick a word (Rust component), shout it (Wavelet), run it (Wavelet CLI entry).

The Rust side is an ordinary `cargo component` crate exporting:

```wit
package acme:words@1.0.0;

interface picker {
  pick: func(seed: u64) -> result<string, string>;
}
```

```
// caps.wvl
Package "demo:caps@0.1.0"

Export shout
Def shout Fn {phrase: string}
  str-cat(upper(phrase) "!")
```

```
// main.wvl
Package "demo:main@0.1.0"
Target "wasi:cli/command"

Import {pkg: "acme:words/picker@1.0.0" as: words}
Import {pkg: "demo:caps/api"           as: caps}

Export run
Def run Fn {}
  Match words/pick({seed: 42}) [
    (ok(w)  println(caps/shout({phrase: w})))
    (err(e) println(str-cat("no word today: " e)))
  ]
```

```console
$ cargo component build --release          # acme_words.wasm
$ wavelet build caps.wvl main.wvl
$ wavelet compose out/*.wasm target/release/acme_words.wasm -o app.wasm
$ wasmtime app.wasm
BANANA!
```

`main.wvl` cannot tell which of its two imports is foreign. That is the point.

---

## 9. Implementation notes

The compiler pipeline is **read → expand → analyze → emit → componentize**. The reader produces form trees (and is where all sugar dies); the expander runs macros to fixpoint, instantiating macro components on demand; analysis resolves bindings, classifies tail positions, computes closure captures, and checks boundary types; emission targets core wasm using GC types (structs for closures, conses, and forms) with `return_call` for tail positions, and a reference-counted linear-memory backend as fallback for GC-less hosts; `wasm-tools` then wraps the core module with canonical-ABI lift/lower into a component matching the synthesized world.

The REPL is a scratch component that is rebuilt and re-composed per definition; since values print as WAVE and code *is* WAVE, the REPL's output is always valid input. Diagnostics ride the `spans` table that travels with every `tree`.

The bootstrap goal is self-hosting in stages: reader and expander rewritten in Wavelet early (they are the best stress test of the meta interface), backend last.

---

## 10. Open questions

Hygiene is the largest: `gensym` suffices for now, but a syntax-object layer over `wavelet:meta/code` (adding scope sets to nodes) is sketched and would slot in without changing the wire type's shape, only extending it. Async maps naturally onto the Component Model's `stream`/`future` types as WASI 0.3 lands; the intent is that a `Fn` whose body awaits compiles to an async-lifted export, with no surface syntax beyond a `Await` macro, but this is undesigned. Pattern exhaustiveness checking at `Match` is possible wherever the scrutinee's boundary type is known and is currently only a lint. And if the canonical ABI grows direct GC-type passing, the `--fuse` optimization becomes less necessary and boundary copies cheaper — a happy problem.

---

## Appendix A — surface grammar

```
file      := form*
form      := primary ("." name payload)*      // call chaining: recv as first arg
primary   := atom | name | qname | call | tuple
           | list | record | flags | macroform
atom      := bool | int | float | char | string
name      := kebab-label                      // WIT identifier, % escape allowed
qname     := name "/" name
call      := (name | qname | title) payload   // payload ATTACHED: no whitespace
payload   := "(" form* ")"                    // only `(` attaches now
tuple     := "(" form* ")"                    // the call form; also a tuple value under Quote
list      := "[" form* "]"
record    := "{" (name ":" form)* "}"
flags     := "{" name* "}"
macroform := title form{arity(title)}         // title := TitleCase token
ws        := space | tab | newline | ","      // comments: "//" to eol
```

(A parenthesized form is one node — the call/tuple node. `(a)` is a zero-argument call of `a`, not transparent grouping, and `[`/`{` no longer attach to a head.)

## Appendix B — design ledger

Decisions with their costs, acknowledged: calls are WAVE tuples with the head first (`(f, x, y)`), not variant cases — so a parenthesized form in evaluation position is always a call and a literal tuple value is written with `Quote`; the list/record call sugar (`f[…]`, `f{…}`) is gone in exchange for one uniform call node. WIT identifiers preclude operator names, so arithmetic is spelled out (`add`, not `+`). Fixed macro arity buys paren-free macro syntax at the price of define-before-use and list-wrapped variadics. The arena encoding of code is uglier than a recursive type, but it is what WIT can express today, and Wavelet hides it everywhere except the wire. Dynamic typing in the core keeps the language small and the boundary types meaningful, at the cost of some errors surfacing at the edge rather than at the keystroke — annotations claw that back where it matters. And per-file components cost boundary copies, which `--fuse` recovers when the whole graph is yours.
