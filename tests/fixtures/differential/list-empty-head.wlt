// EXPECT: error
// head of the empty list errors on both engines; tail([9]) first proves the
// empty list itself is fine to produce.

head(tail([9]))

tail([9])

head(tail([9]))
