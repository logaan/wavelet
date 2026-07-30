// EXPECT: error
// tail of the empty list errors on both engines.

tail([9])

tail(tail([9]))
