// EXPECT: error
// In-range get succeeds; the out-of-bounds index errors on both engines.

get([10 20 30] 0)

get([10 20 30] 2)

get([10 20 30] 3)
