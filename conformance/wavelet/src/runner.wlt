Package "conformance:wavelet@0.1.0"

// Wavelet as the CALLER of the roundtrip:suite conformance world: imports
// values + resources, exports the runner. Seeds and expected responses are
// both literals (chosen within wavelet's expressible range).
//
// Every check the suite defines for the caller role is present below.
//
// All present checks pass against a correct callee. (list-u8-rt, option-u8-rt
// some, and result-tuple-direction-rt ok used to fail to a backend bug —
// byte-width payloads corrupted on lift — fixed in emit.rs and pinned by
// tests/backend_byte_width.rs. Goal 4 made dep variant/enum cases
// constructible (t/circle(1.0), t/north — 4.1) and payload-less results
// spellable and constructible (result-rt, the err side of result-u32-rt,
// the ok side of result-string-err-rt — 4.2).)

Import {pkg: "roundtrip:suite/types" as: t}
Import {pkg: "roundtrip:suite/values" as: iv}
Import {pkg: "roundtrip:suite/resources" as: ir}

// A check yields "" on pass or its name on failure; collect keeps the names.
Def check Fn {name pass}
  If pass "" name

Def collect Fn {xs acc}
  If eq(xs [])
     acc
     Let {h: head(xs)
          acc2: If eq(h "") acc str-cat(str-cat(acc " ") h)}
       collect(tail(xs) acc2)

Def values-fails Fn {}
  Let {u1: iv/no-result(5)
       u2: iv/no-params-no-result()}
    collect([check("bool-rt" eq(iv/bool-rt(true) false))
             check("s8-rt" eq(iv/s8-rt(-5) -4))
             check("s16-rt" eq(iv/s16-rt(-300) -299))
             check("s32-rt" eq(iv/s32-rt(-70000) -69999))
             check("s64-rt" eq(iv/s64-rt(-5000000000) -4999999999))
             check("u8-rt" eq(iv/u8-rt(250) 251))
             check("u16-rt" eq(iv/u16-rt(65000) 65001))
             check("u32-rt" eq(iv/u32-rt(4000000000) 4000000001))
             check("u64-rt" eq(iv/u64-rt(9007199254740991) 9007199254740992))
             check("f64-rt" eq(iv/f64-rt(-0.75) 0.25))
             check("f32-rt" eq(iv/f32-rt(-0.75) 0.25))
             check("char-rt" eq(iv/char-rt('a') 'b'))
             check("string-rt" eq(iv/string-rt("wave") "wave!"))
             check("list-u8-rt" eq(iv/list-u8-rt([1 2 3]) [2 3 4]))
             check("list-string-rt" eq(iv/list-string-rt(["a" ""]) ["a!" "!"]))
             check("list-list-u8-rt" eq(iv/list-list-u8-rt([[1] []]) [[2] []]))
             check("option-u8-rt some" eq(iv/option-u8-rt(some(7)) some(8)))
             check("option-u8-rt none" eq(iv/option-u8-rt(none) none))
             check("option-shape-rt some"
                   eq(iv/option-shape-rt(some(t/dot)) some(t/circle(1.0))))
             check("shape-rt circle"
                   eq(iv/shape-rt(t/circle(1.5)) t/circle(2.5)))
             check("shape-rt rect"
                   eq(iv/shape-rt(t/rect({x: 1 y: 2})) t/rect({x: 2 y: 3})))
             check("shape-rt labelled"
                   eq(iv/shape-rt(t/labelled("a")) t/labelled("a!")))
             check("direction-rt" eq(iv/direction-rt(t/north) t/east))
             check("direction-rt wrap" eq(iv/direction-rt(t/west) t/north))
             check("result-rt ok->err" eq(iv/result-rt(ok()) err()))
             check("result-rt err->ok" eq(iv/result-rt(err()) ok()))
             check("result-u32-rt ok" eq(iv/result-u32-rt(ok(3)) ok(4)))
             check("result-u32-rt err" eq(iv/result-u32-rt(err()) err()))
             check("result-string-err-rt ok"
                   eq(iv/result-string-err-rt(ok()) ok()))
             check("result-string-err-rt err"
                   eq(iv/result-string-err-rt(err("bad")) err("bad!")))
             check("result-u32-string-rt ok"
                   eq(iv/result-u32-string-rt(ok(9)) ok(10)))
             check("result-u32-string-rt err"
                   eq(iv/result-u32-string-rt(err("no")) err("no!")))
             check("result-tuple-direction-rt ok"
                   eq(iv/result-tuple-direction-rt(ok(Quote (1 2))) ok(Quote (2 3))))
             check("result-tuple-direction-rt err"
                   eq(iv/result-tuple-direction-rt(err(t/south)) err(t/west)))
             check("tuple-rt" eq(iv/tuple-rt(Quote (1 "x" true)) Quote (2 "x!" false)))
             check("tuple-rt bundled" eq(iv/tuple-rt(1 "x" true) Quote (2 "x!" false)))
             check("tuple-nested-rt"
                   eq(iv/tuple-nested-rt(Quote ({x: 1 y: 2} [3])) Quote ({x: 2 y: 3} [4])))
             check("point-rt" eq(iv/point-rt({x: 3 y: 4}) {x: 4 y: 5}))
             check("permissions-rt"
                   eq(iv/permissions-rt({read exec}) {write admin}))
             check("permissions-rt empty"
                   eq(iv/permissions-rt({read write exec admin}) {}))
             check("every-primitive-rt"
                   eq(iv/every-primitive-rt({a: true b: -5 c: -300 d: -70000
                                             e: -5000000000 f: 250 g: 65000
                                             h: 4000000000 i: 9007199254740991
                                             j: -0.75 k: -0.75 l: 'a'
                                             m: "wave"})
                      {a: false b: -4 c: -299 d: -69999 e: -4999999999 f: 251
                       g: 65001 h: 4000000001 i: 9007199254740992 j: 0.25
                       k: 0.25 l: 'b' m: "wave!"}))
             check("points-rt" eq(iv/points-rt([{x: 1 y: 2} {x: 3 y: 4}])
                                  [{x: 2 y: 3} {x: 4 y: 5}]))
             check("awkward-rt" eq(iv/awkward-rt({record: 1 list: "r"})
                                   {record: 2 list: "r!"}))
             check("no-params" eq(iv/no-params() 42))
             check("multi-param" eq(iv/multi-param(1 2 3 4) 11))]
            "")

Def resources-fails Fn {}
  Let {c: ir/counter(5)
       n1: ir/next(c)
       n2: ir/next(c)
       n3: ir/value(c)
       c2: ir/counter(5)
       b1: ir/bump-counter(c2)
       b2: ir/value(c2)}
    collect([check("counter.next 1st" eq(n1 5))
             check("counter.next 2nd" eq(n2 6))
             check("counter.value" eq(n3 7))
             check("counter.sum" eq(ir/value(ir/sum([1 2 3])) 6))
             check("make-counter" eq(ir/value(ir/make-counter(5)) 5))
             check("bump-counter" eq(b1 6))
             check("bump-counter state" eq(b2 6))
             check("take-counter" eq(ir/take-counter(ir/counter(5)) 5))
             check("counter-round-trip"
                   eq(ir/value(ir/counter-round-trip(ir/counter(5))) 6))
             check("counter-to-point"
                   eq(ir/counter-to-point(ir/counter(5)) {x: 5 y: 6}))]
            "")

Def report Fn {fails}
  If eq(fails "") ok() err([fails])

Export {name: run iface: "roundtrip:suite/runner" params: {} result: result(_ list(string))}
Def run Fn {}
  report(str-cat(values-fails() resources-fails()))

Export {name: run-values iface: "roundtrip:suite/runner" params: {} result: result(_ list(string))}
Def run-values Fn {}
  report(values-fails())

Export {name: run-resources iface: "roundtrip:suite/runner" params: {} result: result(_ list(string))}
Def run-resources Fn {}
  report(resources-fails())
