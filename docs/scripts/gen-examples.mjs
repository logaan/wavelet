// Regenerates docs/examples.json — the single source of truth for every
// runnable documentation example. Each example's Wavelet source is authored
// here once; this script runs it through the WebAssembly-compiled interpreter
// and records the expected value / output / error. Both the docs <Playground>
// component and the Rust `tests/examples.rs` suite consume the generated JSON,
// so an example can never drift from what the interpreter actually does.
//
// Run after editing examples or changing the language:
//   1. wasm-pack build --target web --out-dir docs/src/wasm --out-name wavelet
//   2. node docs/scripts/gen-examples.mjs
//   3. cargo test            # locks the new behaviour in
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const WASM = join(here, '..', 'src', 'wasm');
const OUT = join(here, '..', 'examples.json');
const { initSync, _eval } = await import(pathToFileURL(join(WASM, 'wavelet.js')).href);
initSync({ module: readFileSync(join(WASM, 'wavelet_bg.wasm')) });

// ── Single source of truth: every runnable doc example, id → Wavelet source ──
const E = {};
E['hello-shout'] = `str-cat(upper("hello") ", world!")`;
E['map-square'] = `map(Fn {x} mul(x x) [1 2 3 4 5])`;
E['match-result-inc'] = `Match ok(42) [
  (ok(n)   add(n 1))
  (err(e)  0)
]`;
E['gs-hello'] = `str-cat(upper("hello") " from wavelet")`;

E['noffi-shout'] = `// 'sh/shout' (an import) and 'shout' (local) would be called identically.
// Here we just call the local one — the point is there's no glue either way.
Def shout Fn {phrase: string}
  str-cat(upper(phrase) "!")
shout({phrase: "no ffi"})`;
E['typed-byte-add-ok'] = `// A typed parameter is checked at the call boundary.
Def byte-add Fn {a: u8 b: u8}
  add(a b)
byte-add({a: 100 b: 50})`;
E['typed-byte-add-bad'] = `// The same function rejects an out-of-range value.
Def byte-add Fn {a: u8 b: u8}
  add(a b)
byte-add({a: 300 b: 1})`;

E['syntax-commas'] = `// commas optional — these are the same list
[1 2 3]`;
E['syntax-quote-call'] = `Quote delete-file({path: "foo.md" force: true})`;
E['syntax-if-arity'] = `// 'If' has arity 3; it reads the next three forms, no parens needed.
If gt(10 3) "bigger" "smaller"`;

E['values-atoms'] = `["a string" 42 -1.5 true 'x' {read write}]`;
E['values-to-u8-ok'] = `to-u8(200)`;
E['values-to-u8-bad'] = `to-u8(999)`;
E['values-record'] = `{name: "Ada" born: 1815 fields: ["maths" "computing"]}`;
E['values-heterogeneous'] = `[1 "two" true 'x']`;
E['values-options-results'] = `[some(1) none ok("yes") err("nope")]`;
E['values-quote-days'] = `Quote days(30)`;
E['values-eq'] = `eq({a: 1 b: [2 3]} {a: 1 b: [2 3]})`;
E['values-unit-def'] = `Def x 10`;

E['eval-fn-by-name'] = `Def delete-file Fn {path force}
  [path force]
// by name…
delete-file({path: "foo.md" force: true})`;
E['eval-fn-by-order'] = `Def delete-file Fn {path force}
  [path force]
// …and by order
delete-file("foo.md" true)`;
E['eval-fn-scalar'] = `Def square Fn {n} mul(n n)
square(9)`;
E['eval-twice'] = `Def twice Fn {f x} f(f(x))
Def inc Fn {n} add(n 1)
twice(inc 10)`;
E['eval-apply-list'] = `Def ops [Fn {n} add(n 1)  Fn {n} mul(n 2)]
apply(get(ops 1) 21)`;

E['sf-def'] = `Def pi-ish 3.14159
mul(pi-ish 2)`;
E['sf-fn-adder'] = `Def adder Fn {by}
  Fn {n} add(n by)
Def add5 adder(5)
add5(10)`;
E['sf-fn-shout'] = `Def shout Fn {phrase: string}
  str-cat(upper(phrase) "!")
shout("hi")`;
E['sf-if'] = `If lt(2 3) "less" "not less"`;
E['sf-if-nonbool'] = `If 1 "yes" "no"`;
E['sf-let'] = `Let {radius: 10
     area: mul(pi mul(radius radius))}
  str-cat("area = " to-string(area))`;
E['sf-do'] = `Do [str-cat("first")
    str-cat("second")
    add(2 2)]`;
E['sf-match'] = `Match [1 2 3] [
  ([]        "empty")
  ([x]       str-cat("one: " to-string(x)))
  ([x y z]   str-cat("three, head " to-string(x)))
  (other     "something else")
]`;
E['sf-quote'] = `Quote add(1 mul(2 3))`;
E['sf-quasi'] = `Let {x: 41}
  Quasi add(Unquote(x) 1)`;
E['sf-unquote'] = `Let {name: Quote(ada)}
  Quasi greeting(Unquote(name))`;
E['sf-splice'] = `Let {middle: [2 3 4]}
  Quasi [1 Splice(middle) 5]`;
E['sf-defmacro'] = `DefMacro unless {cond body}
  Quasi If Unquote(cond) {} Unquote(body)
Let {x: 10}
  Unless gt(x 100) "x is not huge"`;
E['sf-the-ok'] = `The s8 100`;
E['sf-the-bad'] = `The s8 9000`;

E['pm-catch-all'] = `Match 3 [
  (1  "one")
  (2  "two")
  (n  str-cat("many: " to-string(n)))
]`;
E['pm-describe'] = `Def describe Fn {r}
  Match r [
    (ok(v)   str-cat("got " to-string(v)))
    (err(e)  str-cat("failed: " e))
  ]
describe(ok(42))`;
E['pm-none'] = `Match none [
  (none     "nothing")
  (some(v)  to-string(v))
]`;
E['pm-record'] = `Match {name: "Ada" born: 1815 field: "maths"} [
  ({name: n born: y}  str-cat(n " (" to-string(y) ")"))
  (other              "no match")
]`;
E['pm-nested'] = `Match ok([1 2 3]) [
  (ok([first rest-ignored more])  first)
  (ok([])                         0)
  (err(e)                         -1)
]`;
E['pm-no-clause'] = `Match 5 [
  (1  "one")
  (2  "two")
]`;

E['macro-swap'] = `DefMacro swap {a b}
  Quasi [Unquote(b) Unquote(a)]
Let {x: 1 y: 2}
  Swap x y`;
E['macro-and'] = `DefMacro and {a b}
  Quasi If Unquote(a) Unquote(b) false
Let {x: 5}
  And lt(x 10) gt(x 0)`;
E['macro-expand'] = `DefMacro and {a b}
  Quasi If Unquote(a) Unquote(b) false
expand(Quote And(p q))`;
E['macro-gensym-three'] = `[gensym() gensym() gensym()]`;
E['macro-gensym'] = `Def fresh-pair Fn {}
  eq(gensym() gensym())
fresh-pair()`;
E['macro-trylet'] = `DefMacro try-let {binding body}
  Let {name: rec-key(binding) expr: rec-val(binding)}
    Quasi Match Unquote(expr) [
      (ok(Unquote(name))  Unquote(body))
      (err(e)             err(e))
    ]
Def parse Fn {x}
  TryLet {n: ok(x)}
  ok(add(n 1))
parse(41)`;

E['tail-count-down'] = `Def count-down Fn {n}
  If eq(n 0)
     "liftoff"
     count-down(sub(n 1))     // tail position — constant stack
count-down(100000)`;
E['tail-sum-to'] = `Def sum-to Fn {n acc}
  If eq(n 0)
     acc
     sum-to(sub(n 1) add(acc n))
sum-to(10000 0)`;

E['std-pi'] = `mul(pi 2)`;
E['std-predicates'] = `[eq([1 2] [1 2])  lt(2 3)  ge("b" "a")  not(false)]`;
E['std-arith'] = `[add(2 3)  sub(10 4)  mul(6 7)  div(17 5)  rem(17 5)  neg(8)  abs(-3)  min(4 9)  max(4 9)]`;
E['std-div-zero'] = `div(1 0)`;
E['std-div-float'] = `div(7.0 2)`;
E['std-seq-basics'] = `[len([1 2 3])  head([10 20 30])  tail([10 20 30])  reverse([1 2 3])  range(0 5)]`;
E['std-seq-mutate'] = `Let {xs: [10 20 30]}
  [get(xs 1)  put(xs 1 99)  push(xs 40)  concat(xs [40 50])]`;
E['std-map'] = `map(Fn {x} mul(x x) range(1 6))`;
E['std-filter'] = `filter(Fn {x} gt(x 2) [1 2 3 4 1])`;
E['std-fold'] = `fold(Fn {acc x} add(acc x) 0 [1 2 3 4 5])`;
E['std-zip'] = `zip(["a" "b" "c"] [1 2 3])`;
E['std-strcat'] = `str-cat(upper("ada") " " "Lovelace")`;
E['std-strings'] = `[split("a,b,c" ",")  join(["x" "y" "z"] "-")  contains("hello" "ell")]`;
E['std-strcat-tostring'] = `str-cat("count = " to-string(42))`;
E['std-tostring'] = `to-string([1 some(2) {k: "v"}])`;
E['std-read'] = `read("add(1 2)")`;
E['std-conv'] = `[to-u8(255)  to-s8(-128)  to-f64(3)]`;
E['std-conv-bad'] = `to-u8(256)`;
E['std-apply'] = `apply(Fn {a b} add(a b) [20 22])`;
// A quoted call is a tuple now, so 'form-kind(Quote foo(1))' reports "tup"; a
// runtime variant with a payload ('ok(1)') still reports "call".
E['std-form-kind'] = `[form-kind(42)  form-kind("hi")  form-kind(Quote foo)  form-kind(Quote foo(1))  form-kind(ok(1))  form-kind([1 2])]`;
E['std-rec-key-val'] = `Let {b: {text: "hello"}}
  [rec-key(b) rec-val(b)]`;
E['std-constructors'] = `[some(1)  none  ok("yes")  err("nope")]`;
E['std-cells'] = `Let {c: cell-new(0)}
  Do [cell-set(c 41)
      cell-set(c add(cell-get(c) 1))
      cell-get(c)]`;

// ── Run each through the interpreter and record expected results ──
const out = {};
let errCount = 0;
for (const [id, code] of Object.entries(E)) {
  const r = JSON.parse(_eval(code));
  if (r.ok) {
    out[id] = { code, value: r.value, output: r.output };
  } else {
    out[id] = { code, error: r.error };
    errCount++;
  }
}
writeFileSync(OUT, JSON.stringify(out, null, 2) + '\n');
console.log(`wrote ${Object.keys(out).length} examples (${errCount} expected errors) to ${OUT}`);
