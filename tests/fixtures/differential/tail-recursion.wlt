// EXPECT: ok
// Deep self- and mutual tail recursion: the backend's return_call must agree
// with the interpreter's trampoline at depths far beyond any call stack.

Def sum-to Fn {n: s64 acc: s64} The s64
  If le(n 0) acc sum-to(sub(n 1) add(acc n))

Def count-down Fn {n: s64} The s64
  If eq(n 0) 0 count-down(sub(n 1))

Def is-even Fn {n: s64} The bool
  If eq(n 0) true is-odd(sub(n 1))

Def is-odd Fn {n: s64} The bool
  If eq(n 0) false is-even(sub(n 1))

sum-to(100000 0)

count-down(1000000)

[is-even(100001) is-odd(100001)]
