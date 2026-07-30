// EXPECT: ok
// zip/range composition, including unequal lengths (shorter side wins),
// empty and negative ranges, and nested zips.

zip(["a" "b" "c"] [1 2 3])

zip(range(0 3) ["a" "b" "c"])

zip([1 2] ["a" "b" "c"])

zip(["a" "b" "c"] [1 2])

zip(range(0 0) range(0 5))

range(-3 3)

range(3 3)

range(3 0)

reverse(range(0 5))

len(range(0 1000))

zip(range(0 3) zip(range(3 6) range(6 9)))
