Package "conformance:wavelet@0.1.0"

// Wavelet as the CALLEE for the roundtrip:suite `values` interface (5.12).
// Exports every `*-rt` data-type round-trip plus the four function-shape
// functions, so a rust caller's `run-values()` can drive them. The `resources`
// interface is out of scope here (built by roundtrip-resources.wlt); the harness
// terminates the caller's `resources` import with the exports-only stub and
// invokes `run-values()`.
//
// Semantics mirror the suite's documented transforms (wit/deps/.../world.wit).
// Every exported function is signature-annotated (typed params + `The` result)
// so WIT synthesis derives exactly the interface's declared shape.
//
// STATUS (5.12): WIT synthesis + type checking succeed for ALL 35 exported
// functions, and every scalar / char / string / record / tuple / variant /
// option / result transform COMPILES on the wasm backend (char uses real
// next-scalar arithmetic via to-u32/to-char, so the old char-identity gap is
// closed). The remaining blocker is the wasm backend, which does not yet
// implement the higher-order list builtins `map`/`filter`/`fold`, the
// list-building primitives `push`/`concat` (so a manual recursive rebuild is
// not possible either), or `flg`/`contains`. Those are used only by the list
// and flags functions:
//   map      -> list-u8-rt, list-string-rt, list-list-u8-rt, tuple-nested-rt,
//               points-rt
//   filter, contains, flg -> permissions-rt
// Because a `values` export is all-or-nothing (the whole interface or nothing),
// this file does not build until the backend gains those list operations. Once
// it does, `test-values-callee.sh` (to be added, parallel to
// test-resources-callee.sh) plugs it under a rust caller driven by
// `run-values()`.

Import {pkg: "roundtrip:suite/types" as: t}

// --- transform helpers (the documented transforms) ---

// Integer bumps wrap at each width; Wavelet ints are s64 so wrapping is per type.
Def wrap-u8 Fn {n} If eq(n 255) 0 add(n 1)
Def wrap-u16 Fn {n} If eq(n 65535) 0 add(n 1)
Def wrap-u32 Fn {n} If eq(n 4294967295) 0 add(n 1)
Def wrap-u64 Fn {n} add(n 1)
Def wrap-s8 Fn {n} If eq(n 127) -128 add(n 1)
Def wrap-s16 Fn {n} If eq(n 32767) -32768 add(n 1)
Def wrap-s32 Fn {n} If eq(n 2147483647) -2147483648 add(n 1)
Def wrap-s64 Fn {n} If eq(n 9223372036854775807) -9223372036854775808 add(n 1)

Def bump-str Fn {s} str-cat(s "!")

// Next Unicode scalar value: skip the surrogate gap (U+D7FF -> U+E000),
// U+10FFFF wraps to U+0000.
Def next-char Fn {v: char}
  Let {n: to-u32(v)}
    If eq(n 1114111) to-char(0)
    If eq(n 55295)   to-char(57344)
    to-char(add(n 1))

Def bump-point Fn {p}
  Match p [({x: a y: b} {x: wrap-s32(a) y: wrap-s32(b)})]

Def bump-shape Fn {s}
  If eq(s t/dot)
     t/circle(1.0)
     Match s [
       (circle(r)   t/circle(add(r 1.0)))
       (rect(p)     t/rect(bump-point(p)))
       (labelled(x) t/labelled(bump-str(x)))
     ]

Def bump-direction Fn {d}
  If eq(d t/north) t/east
  If eq(d t/east)  t/south
  If eq(d t/south) t/west
  t/north

// --- primitives ---

Export {name: bool-rt iface: "roundtrip:suite/values"}
Def bool-rt Fn {v: bool} The bool not(v)

Export {name: s8-rt iface: "roundtrip:suite/values"}
Def s8-rt Fn {v: s8} The s8 wrap-s8(v)

Export {name: s16-rt iface: "roundtrip:suite/values"}
Def s16-rt Fn {v: s16} The s16 wrap-s16(v)

Export {name: s32-rt iface: "roundtrip:suite/values"}
Def s32-rt Fn {v: s32} The s32 wrap-s32(v)

Export {name: s64-rt iface: "roundtrip:suite/values"}
Def s64-rt Fn {v: s64} The s64 wrap-s64(v)

Export {name: u8-rt iface: "roundtrip:suite/values"}
Def u8-rt Fn {v: u8} The u8 wrap-u8(v)

Export {name: u16-rt iface: "roundtrip:suite/values"}
Def u16-rt Fn {v: u16} The u16 wrap-u16(v)

Export {name: u32-rt iface: "roundtrip:suite/values"}
Def u32-rt Fn {v: u32} The u32 wrap-u32(v)

Export {name: u64-rt iface: "roundtrip:suite/values"}
Def u64-rt Fn {v: u64} The u64 wrap-u64(v)

Export {name: f32-rt iface: "roundtrip:suite/values"}
Def f32-rt Fn {v: f32} The f32 add(v 1.0)

Export {name: f64-rt iface: "roundtrip:suite/values"}
Def f64-rt Fn {v: f64} The f64 add(v 1.0)

Export {name: char-rt iface: "roundtrip:suite/values"}
Def char-rt Fn {v: char} The char next-char(v)

Export {name: string-rt iface: "roundtrip:suite/values"}
Def string-rt Fn {v: string} The string bump-str(v)

// --- built-in compounds ---

Export {name: list-u8-rt iface: "roundtrip:suite/values"}
Def list-u8-rt Fn {v: list(u8)} The list(u8) map(wrap-u8 v)

Export {name: list-string-rt iface: "roundtrip:suite/values"}
Def list-string-rt Fn {v: list(string)} The list(string) map(bump-str v)

Export {name: list-list-u8-rt iface: "roundtrip:suite/values"}
Def list-list-u8-rt Fn {v: list(list(u8))} The list(list(u8))
  map(Fn {xs} map(wrap-u8 xs) v)

Export {name: option-u8-rt iface: "roundtrip:suite/values"}
Def option-u8-rt Fn {v: option(u8)} The option(u8)
  Match v [
    (some(n) some(wrap-u8(n)))
    (none()  none())
  ]

Export {name: option-shape-rt iface: "roundtrip:suite/values"}
Def option-shape-rt Fn {v: option(shape)} The option(shape)
  Match v [
    (some(s) some(bump-shape(s)))
    (none()  none())
  ]

Export {name: result-rt iface: "roundtrip:suite/values"}
Def result-rt Fn {v: result(_ _)} The result(_ _)
  Match v [
    (ok()  err())
    (err() ok())
  ]

Export {name: result-u32-rt iface: "roundtrip:suite/values"}
Def result-u32-rt Fn {v: result(u32 _)} The result(u32 _)
  Match v [
    (ok(n) ok(wrap-u32(n)))
    (err() err())
  ]

Export {name: result-string-err-rt iface: "roundtrip:suite/values"}
Def result-string-err-rt Fn {v: result(_ string)} The result(_ string)
  Match v [
    (ok()   ok())
    (err(s) err(bump-str(s)))
  ]

Export {name: result-u32-string-rt iface: "roundtrip:suite/values"}
Def result-u32-string-rt Fn {v: result(u32 string)} The result(u32 string)
  Match v [
    (ok(n)  ok(wrap-u32(n)))
    (err(s) err(bump-str(s)))
  ]

Export {name: result-tuple-direction-rt iface: "roundtrip:suite/values"}
Def result-tuple-direction-rt Fn {v: result(tuple(u8 u8) direction)}
  The result(tuple(u8 u8) direction)
  Match v [
    (ok(p)  ok(Match p [((a b) tup(wrap-u8(a) wrap-u8(b)))]))
    (err(d) err(bump-direction(d)))
  ]

Export {name: tuple-rt iface: "roundtrip:suite/values"}
Def tuple-rt Fn {v: tuple(u8 string bool)} The tuple(u8 string bool)
  Match v [((n s b) tup(wrap-u8(n) bump-str(s) not(b)))]

Export {name: tuple-nested-rt iface: "roundtrip:suite/values"}
Def tuple-nested-rt Fn {v: tuple(point list(u8))} The tuple(point list(u8))
  Match v [((p xs) tup(bump-point(p) map(wrap-u8 xs)))]

// --- user-defined types ---

Export {name: point-rt iface: "roundtrip:suite/values"}
Def point-rt Fn {v: point} The point bump-point(v)

Export {name: every-primitive-rt iface: "roundtrip:suite/values"}
Def every-primitive-rt Fn {v: every-primitive} The every-primitive
  Match v [
    ({a: pa b: pb c: pc d: pd e: pe f: pf g: pg h: ph i: pi j: pj k: pk l: pl m: pm}
     {a: not(pa) b: wrap-s8(pb) c: wrap-s16(pc) d: wrap-s32(pd) e: wrap-s64(pe)
      f: wrap-u8(pf) g: wrap-u16(pg) h: wrap-u32(ph) i: wrap-u64(pi)
      j: add(pj 1.0) k: add(pk 1.0) l: next-char(pl) m: bump-str(pm)})
  ]

Export {name: awkward-rt iface: "roundtrip:suite/values"}
Def awkward-rt Fn {v: awkward} The awkward
  Match v [({record: r list: l} {record: wrap-u32(r) list: bump-str(l)})]

Export {name: shape-rt iface: "roundtrip:suite/values"}
Def shape-rt Fn {v: shape} The shape bump-shape(v)

Export {name: direction-rt iface: "roundtrip:suite/values"}
Def direction-rt Fn {v: direction} The direction bump-direction(v)

Export {name: permissions-rt iface: "roundtrip:suite/values"}
Def permissions-rt Fn {v: permissions} The permissions
  flg(filter(Fn {n} not(contains(v n)) ["read" "write" "exec" "admin"]))

Export {name: points-rt iface: "roundtrip:suite/values"}
Def points-rt Fn {v: points} The points map(bump-point v)

// --- function shapes ---

Export {name: no-params iface: "roundtrip:suite/values"}
Def no-params Fn {} The u32 42

Export {name: no-result iface: "roundtrip:suite/values"}
Def no-result Fn {v: u32} drop(v)

Export {name: no-params-no-result iface: "roundtrip:suite/values"}
Def no-params-no-result Fn {} drop(0)

Export {name: multi-param iface: "roundtrip:suite/values"}
Def multi-param Fn {a: u8 b: u16 c: u32 d: u64} The u64
  add(add(add(a b) c) add(d 1))
