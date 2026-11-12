import {importComponentBindings as importModule} from './node-component-loader.mjs';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';
import {createCompilerRuntime, formatError} from '../src/runtime/compilerRuntime.mjs';
import {instantiateComponent} from '../src/runtime/componentLoader.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const WASM = join(here, '..', 'src', 'wasm');
const OUT = join(here, '..', 'examples.json');
const { initSync, _eval } = await import(pathToFileURL(join(WASM, 'wavelet.js')).href);
initSync({ module: readFileSync(join(WASM, 'wavelet_bg.wasm')) });

const compiler = await createCompilerRuntime(readFileSync(join(here, '..', 'src/compiler/wavelet_compiler.wasm')), {
  instantiate: (bytes, imports) => instantiateComponent(bytes, imports, importModule),
});

const E = {};
E['hello-shout'] = `str-cat("hello" ", world!")`;
E['map-square'] = `map(Fn {x} mul(x x) [1 2 3 4 5])`;
E['match-result-inc'] = `Match ok(42) [
  (ok(n)   add(n 1))
  (err(e)  0)
]`;
E['gs-hello'] = `str-cat("hello" " from wavelet")`;

E['noffi-shout'] = `Def shout Fn {phrase: string}
  str-cat(phrase "!")
shout({phrase: "no ffi"})`;
E['typed-byte-add-ok'] = `Def byte-add Fn {a: u8 b: u8}
  add(a b)
byte-add({a: 100 b: 50})`;
E['typed-byte-add-bad'] = `Def byte-add Fn {a: u8 b: u8}
  add(a b)
byte-add({a: 300 b: 1})`;

E['syntax-commas'] = `[1 2 3]`;
E['syntax-quote-call'] = `Quote delete-file({path: "foo.md" force: true})`;
E['syntax-if-arity'] = `If gt(10 3) "bigger" "smaller"`;
E['syntax-chain'] = `"hello".str-cat(", world").str-cat("!")`;

E['values-atoms'] = `{s: "a string" n: 42 d: -1.5 b: true c: 'x' f: {read write}}`;
E['values-to-u8-ok'] = `to-u8(200)`;
E['values-to-u8-bad'] = `to-u8(999)`;
E['values-record'] = `{name: "Ada" born: 1815 fields: ["maths" "computing"]}`;
E['values-heterogeneous'] = `[1 "two" true 'x']`;
E['values-tuple'] = `Let {p: tuple2("abc" 123)}
  Match p [((s n) tuple3(s n true))]`;
E['values-options-results'] = `{options: [some(1) none] results: [ok("yes") err("nope")]}`;

E['values-payloadless-results'] = `Def flip Fn {v: result}
  Match v [((ok) err()) ((err) ok())]
flip(ok())`;
E['values-quote-days'] = `Quote days(30)`;
E['values-eq'] = `eq({a: 1 b: [2 3]} {a: 1 b: [2 3]})`;
E['values-unit-def'] = `Def x 10`;

E['eval-fn-by-name'] = `Def delete-file Fn {path: string force: bool}
  tuple2(path force)
delete-file({path: "foo.md" force: true})`;
E['eval-fn-by-order'] = `Def delete-file Fn {path: string force: bool}
  tuple2(path force)
delete-file("foo.md" true)`;
E['eval-fn-scalar'] = `Def square Fn {n} mul(n n)
square(9)`;
E['eval-twice'] = `DefMacro twice {f x} Quasi (Unquote(f) (Unquote(f) Unquote(x)))
Def inc Fn {n: s64} add(n 1)
Twice inc 10`;
E['eval-apply-list'] = `DefType operation [increment double]
Def ops [increment double]
Match get(ops 1) [(increment add(21 1)) (double mul(21 2))]`;

E['sf-def'] = `Def pi-ish 3.14159
mul(pi-ish 2)`;

E['sf-deftype-cases'] = `DefType ttl [days(u32) forever]
Def describe Fn {t: ttl}
  Match t [((days n) str-cat("expires in " to-string(n) " days"))
           (forever "never expires")]
[describe(days(30)) describe(forever)]`;
E['sf-fn-adder'] = `Def add5 Fn {n: s64} add(n 5)
add5(10)`;
E['sf-fn-shout'] = `Def shout Fn {phrase: string}
  str-cat(phrase "!")
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
  Quasi If Unquote(cond) "" Unquote(body)
Let {x: 10}
  Unless gt(x 100) "x is not huge"`;
E['sf-the-ok'] = `The s8 100`;
E['sf-the-bad'] = `The s8 9000`;

E['pm-catch-all'] = `Match 3 [
  (1  "one")
  (2  "two")
  (n  str-cat("many: " to-string(n)))
]`;
E['pm-describe'] = `Def describe Fn {r: result(s64 string)}
  Match r [
    (ok(v)   str-cat("got " to-string(v)))
    (err(e)  str-cat("failed: " e))
  ]
describe(ok(42))`;
E['pm-none'] = `Match The option(string) none [
  (none     "nothing")
  (some(v)  to-string(v))
]`;
E['pm-record'] = `Match {name: "Ada" born: 1815 field: "maths"} [
  ({name: n born: y}  str-cat(n " (" to-string(y) ")"))
  (other              "no match")
]`;
E['pm-nested'] = `Match ok([1 2 3]) [
  (ok([first rest-ignored more])  first)
  (ok(other)                      0)
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
DefMacro inspect {form}
  Quasi Quote Unquote(expand(form))
Inspect And(p q)`;
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
Def parse Fn {x: s64}
  Try-let {n: The result(s64 string) ok(x)}
  ok(add(n 1))
parse(41)`;

E['tail-count-down'] = `Def count-down Fn {n}
  If eq(n 0)
     "liftoff"
     count-down(sub(n 1))
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
E['std-seq-basics'] = `{len: len([1 2 3]) head: head([10 20 30]) tail: tail([10 20 30]) reverse: reverse([1 2 3]) range: range(0 5)}`;
E['std-seq-mutate'] = `Let {xs: [10 20 30]}
  {get: get(xs 1) put: put(xs 1 99) push: push(xs 40) concat: concat(xs [40 50])}`;
E['std-map'] = `map(Fn {x} mul(x x) range(1 6))`;
E['std-filter'] = `filter(Fn {x} gt(x 2) [1 2 3 4 1])`;
E['std-fold'] = `fold(Fn {acc x} add(acc x) 0 [1 2 3 4 5])`;
E['std-zip'] = `zip(["a" "b" "c"] [1 2 3])`;
E['std-tuple'] = `{empty: tuple0() single: tuple1("only") pair: tuple2(1 "a") nested: tuple2(true tuple2(2 3))}`;
E['std-strcat'] = `str-cat("Ada" " " "Lovelace")`;
E['std-strings'] = `{split: split("a,b,c" ",") join: join(["x" "y" "z"] "-") contains: contains("hello" "ell")}`;
E['std-strcat-tostring'] = `str-cat("count = " to-string(42))`;
E['std-tostring'] = `to-string({n: 1 opt: some(2) rec: {k: "v"}})`;
E['std-conv'] = `{byte: to-u8(255) signed: to-s8(-128) float: to-f64(3)}`;
E['std-conv-bad'] = `to-u8(256)`;
E['std-char-conv'] = `{code: to-u32('a') char: to-char(98) next: to-char(add(to-u32('a') 1))}`;
E['std-char-conv-bad'] = `to-char(55296)`;

E['std-form-kind'] = `[form-kind(42)  form-kind("hi")  form-kind(Quote foo)  form-kind(Quote foo(1))  form-kind(ok(1))  form-kind([1 2])]`;
E['std-rec-key-val'] = `Let {b: {text: "hello"}}
  {key: rec-key(b) val: rec-val(b)}`;
E['std-constructors'] = `{options: [some(1) none] results: [ok("yes") err("nope")]}`;
E['std-cells'] = `Let {c: cell-new(0)}
  Do [cell-set(c 41)
      cell-set(c add(cell-get(c) 1))
      cell-get(c)]`;

E['ty-overload-int'] = `Def show Fn {x: s32} "int"
Def show Fn {x: string} "str"
show(5)`;
E['ty-overload-str'] = `Def show Fn {x: string} "str"
Def show Fn {x: s32} "int"
show("hi")`;
E['ty-the-return'] = `Def make Fn {} 1
Def make Fn {} "x"
The s64 make()`;
E['ty-the-range'] = `The u8 300`;
E['ty-mixed-add'] = `Def bad Fn {x: s32} add(x "y")`;

E['ty-user-functor'] = `DefFunctor "demo:pair" {left right} [
  DefType pair tuple(left right)
  Def make Fn {x: left y: right} The pair tuple2(x y)
]
Instantiate {pkg: "demo:pair" with: {left: s64 right: string} as: mixed}
mixed-make(7 "seven")`;

E['res-counter'] = `DefResource counter {
  New: Fn {start: u32} cell-new(start)
  next: Fn {self: counter} The u32
    Let {value: cell-get(self)} Do [cell-set(self add(value 1)) value]
  value: Fn {self: counter} The u32 cell-get(self)
}
Let {c: counter(10)} Do [counter-next(c) counter-next(c) counter-value(c)]`;

E['tour-values'] = `// 1. Values, names, and calls
// Comments start with //. Commas and newlines separate forms like spaces.
Def answer 42
Def label "wavelet"
Def letter 'W'
Def ready true

// Functions use names instead of punctuation operators.
add(answer 8)                    // 50
"hello".str-cat("!")

// Lists have one element type. Records and tuples can mix types.
Def numbers [1, 2, 3]
Def point {x: 3 y: 4}
Def pair tuple2(label answer)    // Parentheses alone would mean a call.

// Integer literals default to s64, floats to f64. Context can narrow them.
The u8 200
// The u8 300 would be a compile error. to-u8(value) checks at runtime.

tuple3(ready len(numbers) pair)  // (true, 3, ("wavelet", 42))`;

E['tour-functions'] = `// 2. Functions and local bindings
// Def creates an immutable binding. Fn supplies parameters and a body.
Def greet Fn {name: string} str-cat("Hello, " name "!")
greet("Ada")                    // "Hello, Ada!"
greet({name: "Ada"})            // Named arguments use a record.

Def total Fn {a: s64 b: s64} add(a b)
total(20 22)                    // Multiple positional arguments form a tuple.

// Let bindings are sequential: the later ones can use earlier ones.
Let {base: 40 extra: add(base 1)} add(extra 1)

// Callbacks and captures resolve during compilation.
// Functions cannot be stored in runtime collections or returned as closures.
Let {offset: 10}
  map(Fn {n} add(n offset) [1 2 3])  // [11, 12, 13]`;

E['tour-branches'] = `// 3. Branches, records, and variants
// If requires bool; both branches must have compatible result types.
If lt(2 3) "smaller" "larger"

DefType point {x: s64 y: s64}
DefType shape [dot(point) empty]

// Match checks coverage at compile time. Patterns bind their payloads.
Def describe Fn {s: shape}
  Match s [
    (dot({x: x y: y}) str-cat(to-string(x) "," to-string(y)))
    (empty "empty")
  ]
describe(dot({x: 3 y: 4}))        // "3,4"

// Options and results make absence and failure explicit.
Def fallback Fn {value: option(s64)}
  Match value [(some(n) n) (none 0)]
fallback(none)                   // 0
Match ok(42) [(ok(n) n) (err(e) 0)]  // 42`;

E['tour-collections'] = `// 4. Collections and tail recursion
map(Fn {n} mul(n n) [1 2 3])     // [1, 4, 9]
filter(Fn {n} gt(n 2) [1 2 3 4]) // [3, 4]
fold(Fn {acc n} add(acc n) 0 [1 2 3 4])           // 10

// Tail calls within a component reuse the stack frame.
Def sum-to Fn {n: s64 acc: s64}
  If eq(n 0) acc sum-to(sub(n 1) add(acc n))
sum-to(100 0)                    // 5050`;

E['tour-resources'] = `// 5. State and resources
// Do evaluates in order and returns the final value.
Let {c: cell-new(0)}
  Do [cell-set(c 2) cell-get(c)]  // 2

// A resource hides its state behind a handle and methods.
DefResource counter {
  New: Fn {start: u32} cell-new(start)
  next: Fn {self: counter}
    The u32 Do [cell-set(self add(cell-get(self) 1)) cell-get(self)]
  value: Fn {self: counter} The u32 cell-get(self)
}

// A local function borrows an existing handle.
Def peek Fn {c: borrow(counter)} The u32 counter/value(c)
Let {c: counter(5)}
  Do [counter/next(c) peek(c)]    // 6

// At a component boundary, counter or own(counter) transfers ownership.
// borrow(counter) lets a callee use the handle without taking ownership.
// drop(value) discards a value; it does not call a resource destructor.`;

E['tour-macros'] = `// 6. Code as data and compile-time macros
Quote add(1 2)                  // The source form, without running the call.

// Quasi builds a form. Unquote inserts a value into that form.
Let {n: 5} Quasi add(Unquote(n) 1)

// Macro parameters receive unevaluated forms.
// Define macros before use; TitleCase syntax consumes their known arity.
DefMacro twice {expression}
  Quasi add(Unquote(expression) Unquote(expression))
Twice 21                        // 42
// This macro evaluates its expression twice. Expansion is unhygienic.`;

E['tour-types'] = {
  compiledOnly: true,
  code: `// 7. Concrete types and compile-time specialization
// Overloads resolve from argument types or an expected result type.
Def describe Fn {n: s64} str-cat("number " to-string(n))
Def describe Fn {s: string} str-cat("text " s)
describe(7)                     // "number 7"

// A functor creates concrete declarations for its type arguments.
Instantiate {pkg: "wavelet:coll/set" with: {elem: s32} as: ints}
Let {s: ints/new()}
  Do [ints/add(s 2) ints/add(s 2) ints/size(s)] // 1

// User templates are file-local; no generic type crosses the boundary.
DefFunctor "tour:identity" {elem} [
  Def identity Fn {value: elem} value
]
Instantiate {pkg: "tour:identity" with: {elem: string} as: words}
words-identity("concrete")      // "concrete"`,
  value: '"concrete"',
  output: '',
};

E['tour-components'] = {
  sources: [
    {name: 'greeting.wlt', contents: `// 8. One file becomes one component.
Package "tour:greeting@0.1.0"

// Export publishes a function through the default api interface.
Export greet
Def greet Fn {name: string} str-cat("Hello, " name "!")`},
    {name: 'main.wlt', contents: `Package "tour:main@0.1.0"

// Imports name WIT interfaces, independently of implementation language.
Import {pkg: "tour:greeting/api" as: greeting}

Export {name: run result: string}
Def run Fn {} greeting/greet("components")

// Runtime host access, including WASI, also requires explicit imports.
// Macro components use Import {pkg: "..." macros: true from: "...wasm"}.
// Supply their executable artifacts explicitly; WIT alone is only a contract.`},
  ],
  entry: 'main.wlt',
  invocation: {interfaceName: 'tour:main/api@0.1.0', functionName: 'run'},
  value: '"Hello, components!"',
  output: '',
};

const out = {};
let errCount = 0;
for (const [id, example] of Object.entries(E)) {
  if (typeof example !== 'string') {
    const compiled = await compiler.evaluate({...example, source: example.code});
    if (compiled.value !== example.value || compiled.output !== example.output) {
      throw new Error(`${id}: compiled example disagrees with expected result\n${JSON.stringify(compiled)}`);
    }
    out[id] = example;
    continue;
  }
  const code = example;
  const r = JSON.parse(_eval(code));
  let compiled;
  try {
    compiled = await compiler.evaluate({source: code});
  } catch (error) {
    compiled = {ok: false, error: formatError(error)};
  }
  if (compiled.ok !== r.ok || (r.ok && (compiled.value !== r.value || compiled.output !== r.output))) {
    throw new Error(`${id}: compiled example disagrees with interpreter\n${JSON.stringify({interpreter: r, compiled})}`);
  }
  if (r.ok) {
    out[id] = { code, value: r.value, output: r.output };
  } else {
    out[id] = { code, error: r.error };
    errCount++;
  }
}
writeFileSync(OUT, JSON.stringify(out, null, 2) + '\n');
console.log(`wrote ${Object.keys(out).length} examples (${errCount} expected errors) to ${OUT}`);
