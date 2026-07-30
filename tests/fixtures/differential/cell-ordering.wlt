// EXPECT: ok
// Cell mutation ordering: `cell-new`/`cell-get`/`cell-set` sequences inside
// `Do` bodies and list literals, where evaluation order is observable.

Let {c: cell-new(0)}
  Do [cell-set(c 41)
      cell-set(c add(cell-get(c) 1))
      cell-get(c)]

Let {a: cell-new(1) b: cell-new(2)}
  Do [cell-set(a add(cell-get(a) cell-get(b)))
      cell-set(b add(cell-get(a) cell-get(b)))
      [cell-get(a) cell-get(b)]]

Let {c: cell-new(10)}
  [cell-get(c)
   Do [cell-set(c 20) cell-get(c)]
   Do [cell-set(c 30) cell-get(c)]
   cell-get(c)]

Let {c: cell-new(5)} cell-set(c 6)
