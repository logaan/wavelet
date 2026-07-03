Package "conformance:wavelet@0.1.0"

Import {pkg: "roundtrip:suite/types" as: t}

// --- transform helpers (the documented transforms from wit/world.wit) ---

// Integer bumps wrap at each width. Wavelet ints are s64, so wrapping is
// spelled out per type; u64 has no representable MAX to wrap at (known gap).
Def wrap-u8 Fn {n} If eq(n 255) 0 add(n 1)
Def wrap-u16 Fn {n} If eq(n 65535) 0 add(n 1)
Def wrap-u32 Fn {n} If eq(n 4294967295) 0 add(n 1)
Def wrap-u64 Fn {n} add(n 1)
Def wrap-s8 Fn {n} If eq(n 127) -128 add(n 1)
Def wrap-s16 Fn {n} If eq(n 32767) -32768 add(n 1)
Def wrap-s32 Fn {n} If eq(n 2147483647) -2147483648 add(n 1)
Def wrap-s64 Fn {n} If eq(n 9223372036854775807) -9223372036854775808 add(n 1)

Def bump-str Fn {s} str-cat(s "!")

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

// --- values: one export per suite function, callee role ---

Export {name: bool-rt iface: "roundtrip:suite/values"}
Def bool-rt Fn {v} not(v)

Export {name: s8-rt iface: "roundtrip:suite/values"}
Def s8-rt Fn {v} wrap-s8(v)

Export {name: s16-rt iface: "roundtrip:suite/values"}
Def s16-rt Fn {v} wrap-s16(v)

Export {name: s32-rt iface: "roundtrip:suite/values"}
Def s32-rt Fn {v} wrap-s32(v)

Export {name: s64-rt iface: "roundtrip:suite/values"}
Def s64-rt Fn {v} wrap-s64(v)

Export {name: u8-rt iface: "roundtrip:suite/values"}
Def u8-rt Fn {v} wrap-u8(v)

Export {name: u16-rt iface: "roundtrip:suite/values"}
Def u16-rt Fn {v} wrap-u16(v)

Export {name: u32-rt iface: "roundtrip:suite/values"}
Def u32-rt Fn {v} wrap-u32(v)

Export {name: u64-rt iface: "roundtrip:suite/values"}
Def u64-rt Fn {v} wrap-u64(v)

Export {name: f32-rt iface: "roundtrip:suite/values"}
Def f32-rt Fn {v} add(v 1.0)

Export {name: f64-rt iface: "roundtrip:suite/values"}
Def f64-rt Fn {v} add(v 1.0)

// Known gap: no char<->scalar-value conversion in the stdlib, so the
// next-Unicode-scalar transform is inexpressible; identity stands in.
Export {name: char-rt iface: "roundtrip:suite/values"}
Def char-rt Fn {v} v

Export {name: string-rt iface: "roundtrip:suite/values"}
Def string-rt Fn {v} bump-str(v)

Export {name: list-u8-rt iface: "roundtrip:suite/values"}
Def list-u8-rt Fn {v} map(wrap-u8 v)

Export {name: list-string-rt iface: "roundtrip:suite/values"}
Def list-string-rt Fn {v} map(bump-str v)

Export {name: list-list-u8-rt iface: "roundtrip:suite/values"}
Def list-list-u8-rt Fn {v} map(Fn {xs} map(wrap-u8 xs) v)

Export {name: option-u8-rt iface: "roundtrip:suite/values"}
Def option-u8-rt Fn {v}
  Match v [
    (some(n) some(wrap-u8(n)))
    (none()  none())
  ]

Export {name: option-shape-rt iface: "roundtrip:suite/values"}
Def option-shape-rt Fn {v}
  Match v [
    (some(s) some(bump-shape(s)))
    (none()  none())
  ]

Export {name: result-rt iface: "roundtrip:suite/values"}
Def result-rt Fn {v}
  Match v [
    (ok()  err())
    (err() ok())
  ]

Export {name: result-u32-rt iface: "roundtrip:suite/values"}
Def result-u32-rt Fn {v}
  Match v [
    (ok(n) ok(wrap-u32(n)))
    (err() err())
  ]

Export {name: result-string-err-rt iface: "roundtrip:suite/values"}
Def result-string-err-rt Fn {v}
  Match v [
    (ok()   ok())
    (err(s) err(bump-str(s)))
  ]

Export {name: result-u32-string-rt iface: "roundtrip:suite/values"}
Def result-u32-string-rt Fn {v}
  Match v [
    (ok(n)  ok(wrap-u32(n)))
    (err(s) err(bump-str(s)))
  ]

Export {name: result-tuple-direction-rt iface: "roundtrip:suite/values"}
Def result-tuple-direction-rt Fn {v}
  Match v [
    (ok(p)  ok(Match p [((a b) tup(wrap-u8(a) wrap-u8(b)))]))
    (err(d) err(bump-direction(d)))
  ]

Export {name: tuple-rt iface: "roundtrip:suite/values"}
Def tuple-rt Fn {v}
  Match v [((n s b) tup(wrap-u8(n) bump-str(s) not(b)))]

Export {name: tuple-nested-rt iface: "roundtrip:suite/values"}
Def tuple-nested-rt Fn {v}
  Match v [((p xs) tup(bump-point(p) map(wrap-u8 xs)))]

Export {name: point-rt iface: "roundtrip:suite/values"}
Def point-rt Fn {v} bump-point(v)

Export {name: every-primitive-rt iface: "roundtrip:suite/values"}
Def every-primitive-rt Fn {v}
  Match v [
    ({a: pa b: pb c: pc d: pd e: pe f: pf g: pg h: ph i: pi j: pj k: pk l: pl m: pm}
     {a: not(pa) b: wrap-s8(pb) c: wrap-s16(pc) d: wrap-s32(pd) e: wrap-s64(pe)
      f: wrap-u8(pf) g: wrap-u16(pg) h: wrap-u32(ph) i: wrap-u64(pi)
      j: add(pj 1.0) k: add(pk 1.0) l: pl m: bump-str(pm)})
  ]

Export {name: awkward-rt iface: "roundtrip:suite/values"}
Def awkward-rt Fn {v}
  Match v [({record: r list: l} {record: wrap-u32(r) list: bump-str(l)})]

Export {name: shape-rt iface: "roundtrip:suite/values"}
Def shape-rt Fn {v} bump-shape(v)

Export {name: direction-rt iface: "roundtrip:suite/values"}
Def direction-rt Fn {v} bump-direction(v)

Export {name: permissions-rt iface: "roundtrip:suite/values"}
Def permissions-rt Fn {v}
  flg(filter(Fn {n} not(contains(v n)) ["read" "write" "exec" "admin"]))

Export {name: points-rt iface: "roundtrip:suite/values"}
Def points-rt Fn {v} map(bump-point v)

Export {name: no-params iface: "roundtrip:suite/values"}
Def no-params Fn {} 42

Export {name: no-result iface: "roundtrip:suite/values"}
Def no-result Fn {v} 0

Export {name: no-params-no-result iface: "roundtrip:suite/values"}
Def no-params-no-result Fn {} 0

Export {name: multi-param iface: "roundtrip:suite/values"}
Def multi-param Fn {a b c d} add(add(add(a b) c) add(d 1))
