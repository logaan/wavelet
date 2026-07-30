// A functor set handle returned across the boundary, then mutated and queried
// by later method calls on the same live instance. Driven by the
// multicall_set_handle_* script in tests/differential_fixtures.rs.
Package "diff:sethandle@0.1.0"

Instantiate {pkg: "wavelet:coll/set" with: {elem: s32} as: ints}

Export build-ints
Def build-ints Fn {}
  Let {s: ints/new()}
    Do [ints/add(s 1)
        ints/add(s 2)
        ints/add(s 1)
        s]
