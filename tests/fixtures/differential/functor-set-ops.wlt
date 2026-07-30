// EXPECT: ok
// Functor SET_OPS over a primitive element within one call: dedup on add,
// membership, size, and independent handles.

Instantiate {pkg: "wavelet:coll/set" with: {elem: s32} as: ints}

Let {s: ints/new()}
  Do [ints/add(s 4)
      ints/add(s 4)
      ints/add(s 7)
      {size: ints/size(s) has4: ints/contains(s 4) has5: ints/contains(s 5)}]

Let {a: ints/new() b: ints/new()}
  Do [ints/add(a 1)
      {a: ints/size(a) b: ints/size(b) bhas1: ints/contains(b 1)}]

Let {s: ints/new()} ints/add(s 9)

Let {s: ints/new()} {size: ints/size(s) has0: ints/contains(s 0)}
