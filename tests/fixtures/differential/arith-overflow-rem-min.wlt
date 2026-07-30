// EXPECT: error
// i64::MIN rem -1 overflows in the oracle (checked_rem); the backend's
// arith_int traps identically (wasm i64.rem_s alone would yield 0).

rem(-9223372036854775808 1)

rem(-9223372036854775808 -1)
