// EXPECT: error
// put past the end errors on both engines after an in-range put succeeds.

put([1 2 3] 1 99)

put([1 2 3] 3 0)
