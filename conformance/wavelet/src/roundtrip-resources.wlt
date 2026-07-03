Package "conformance:wavelet@0.1.0"

// Wavelet as the CALLEE for the roundtrip:suite `resources` interface (4.5).
// Exports the whole `counter` resource plus the own/borrow free functions, so a
// rust caller's `run-resources()` can drive them. The `values` interface is out
// of scope here (goal-5 representation gaps); the harness terminates the caller's
// `values` import with the exports-only stub and invokes `run-resources()`.
//
// Semantics mirror the suite's documented transforms (wit/deps/.../world.wit and
// suite/src/lib.rs): `next` post-increments, `value` reads without advancing,
// `sum` is an alternative constructor over a wrapping u32 sum, and the free
// functions pass owned / borrowed handles across the boundary. u32 wrapping is
// handled by the boundary's u32 truncation of Wavelet's s64 arithmetic.

Import {pkg: "roundtrip:suite/types" as: t}

DefResource counter {
  New: Fn {start: u32} cell-new(start)
  next: Fn {self: counter} The u32
    Let {v: cell-get(self)}
      Do [cell-set(self add(v 1)) v]
  value: Fn {self: counter} The u32 cell-get(self)
  sum: Static Fn {values: list(u32)} The counter counter(sum-list(values))
}
Export {iface: "roundtrip:suite/resources" name: counter}

Def sum-list Fn {vs}
  If eq(vs []) 0 add(head(vs) sum-list(tail(vs)))

Export {name: make-counter iface: "roundtrip:suite/resources"}
Def make-counter Fn {start: u32} The counter counter(start)

Export {name: bump-counter iface: "roundtrip:suite/resources"}
// Advance once and return the POST-advance value (WIT: "returns the post-advance
// value"). `next` post-increments (returns the pre-advance value), so advance
// with `next` then read the new value with `value`.
Def bump-counter Fn {c: borrow(counter)}
  The u32 Do [counter/next(c) counter/value(c)]

Export {name: take-counter iface: "roundtrip:suite/resources"}
Def take-counter Fn {c: counter} The u32 counter/value(c)

Export {name: counter-round-trip iface: "roundtrip:suite/resources"}
Def counter-round-trip Fn {c: counter} The counter counter(add(counter/value(c) 1))

Export {name: counter-to-point iface: "roundtrip:suite/resources"}
Def counter-to-point Fn {c: borrow(counter)} The point
  Let {x: counter/value(c)}
    {x: x y: add(x 1)}
