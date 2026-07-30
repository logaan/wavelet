// EXPECT: error
// sub below i64::MIN overflows on both engines.

sub(-9223372036854775807 1)

sub(-9223372036854775808 1)
