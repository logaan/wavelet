// A resource whose cell state must persist across separate exported calls on
// one live instance. Driven by the multicall_counter_* script in
// tests/differential_fixtures.rs on both engines.
Package "diff:counter@0.1.0"

DefResource counter {
  New: Fn {start: u32} cell-new(start)
  next: Fn {self: counter} The u32
    Let {v: cell-get(self)}
      Do [cell-set(self add(v 1)) v]
  value: Fn {self: counter} The u32 cell-get(self)
  add-n: Fn {self: counter n: u32} The u32
    Let {v: add(cell-get(self) n)}
      Do [cell-set(self v) v]
  sum: Static Fn {values: list(u32)} The counter counter(sum-list(values))
}
Export counter

Def sum-list Fn {vs: list(u32)} The u32
  If eq(vs []) 0 add(head(vs) sum-list(tail(vs)))
