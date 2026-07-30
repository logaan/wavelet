// EXPECT: ok
// Functor SET_OPS over a Derive'd record element: structural equality decides
// membership, exactly as the interpreter's eq.

DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Instantiate {pkg: "wavelet:coll/set" with: {elem: point} as: pts}

Let {s: pts/new()}
  Do [pts/add(s {x: 1 y: 2})
      pts/add(s {x: 3 y: 4})
      pts/add(s {x: 1 y: 2})
      pts/size(s)]

Let {s: pts/new()}
  Do [pts/add(s {x: 1 y: 2})
      [pts/contains(s {x: 1 y: 2}) pts/contains(s {x: 2 y: 1})]]
