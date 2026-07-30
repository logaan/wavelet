// EXPECT: ok
// Arithmetic edges that stay in range: i64 extremes as values, truncating
// division / remainder signs, and float division.

[add(2 3) sub(2 3) mul(-4 5) neg(8) abs(-3) min(-2 2) max(-2 2)]

[div(17 5) div(-17 5) div(17 -5) rem(17 5) rem(-17 5) rem(17 -5)]

9223372036854775807

add(9223372036854775806 1)

sub(-9223372036854775807 1)

[abs(-9223372036854775807) neg(9223372036854775807)]

div(7.0 2)
