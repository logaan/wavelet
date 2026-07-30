// EXPECT: ok
// Resource lifecycle within one call: New, methods mutating cell state, and
// the Static alternative constructor. (Cross-call persistence is pinned by
// the multicall/counter.wlt script.)

DefResource counter {
  New: Fn {start: u32} cell-new(start)
  next: Fn {self: counter} The u32
    Let {v: cell-get(self)}
      Do [cell-set(self add(v 1)) v]
  value: Fn {self: counter} The u32 cell-get(self)
  sum: Static Fn {values: list(u32)} The counter counter(sum-list(values))
}
Export counter

Def sum-list Fn {vs: list(u32)} The u32
  If eq(vs []) 0 add(head(vs) sum-list(tail(vs)))

Let {c: counter(3)} [counter/next(c) counter/next(c) counter/value(c)]

Let {c: counter/sum([10 20 30])} Do [counter/next(c) counter/value(c)]

Let {a: counter(0) b: counter(100)}
  Do [counter/next(a)
      counter/next(a)
      [counter/value(a) counter/value(b)]]
