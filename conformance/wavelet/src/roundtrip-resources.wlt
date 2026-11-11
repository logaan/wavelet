Package "conformance:wavelet@0.1.0"

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

Def sum-list Fn {vs: list(u32)} The u32
  If eq(vs []) 0 add(head(vs) sum-list(tail(vs)))

Export {name: make-counter iface: "roundtrip:suite/resources"}
Def make-counter Fn {start: u32} The counter counter(start)

Export {name: bump-counter iface: "roundtrip:suite/resources"}

Def bump-counter Fn {c: borrow(counter)}
  The u32 Do [counter/next(c) counter/value(c)]

Export {name: take-counter iface: "roundtrip:suite/resources"}
Def take-counter Fn {c: counter} The u32 counter/value(c)

Export {name: counter-round-trip iface: "roundtrip:suite/resources"}
Def counter-round-trip Fn {c: counter} The counter counter(add(counter/value(c) 1))

Export {name: counter-to-point iface: "roundtrip:suite/resources"}
Def counter-to-point Fn {c: borrow(counter)} The point
  Let {x: to-s32(counter/value(c))}
    {x: x y: add(x 1)}
