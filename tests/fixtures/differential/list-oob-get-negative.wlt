// EXPECT: error
// A negative index is out of bounds on both engines.

get([1 2 3] 1)

get([1 2 3] neg(1))
