// EXPECT: error
// add at i64::MAX overflows: an error on both engines at the same position.

add(9223372036854775806 1)

add(9223372036854775807 1)
