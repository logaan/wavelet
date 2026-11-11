Package "conformance:wavelet@0.1.0"

Import {pkg: "roundtrip:suite/types" as: t}




Def wrap-u8 Fn {n} If eq(n 255) 0 add(n 1)
Def wrap-u16 Fn {n} If eq(n 65535) 0 add(n 1)
Def wrap-u32 Fn {n} If eq(n 4294967295) 0 add(n 1)
Def wrap-u64 Fn {n} add(n 1)
Def wrap-s8 Fn {n} If eq(n 127) -128 add(n 1)
Def wrap-s16 Fn {n} If eq(n 32767) -32768 add(n 1)
Def wrap-s32 Fn {n} If eq(n 2147483647) -2147483648 add(n 1)
Def wrap-s64 Fn {n} If eq(n 9223372036854775807) -9223372036854775808 add(n 1)

Def bump-str Fn {s: string} The string str-cat(s "!")

Def next-char Fn {v: char}
  Let {n: to-u32(v)}
    If eq(n 1114111) to-char(0)
    If eq(n 55295)   to-char(57344)
    to-char(add(n 1))

Def bump-point Fn {p: point} The point
  Match p [({x: a y: b} {x: wrap-s32(a) y: wrap-s32(b)})]

Def bump-shape Fn {s: shape} The shape
  Match s [
       (dot() t/circle(1.0))
       (circle(radius)   t/circle(add(radius 1.0)))
       (rect(p)     t/rect(bump-point(p)))
       (labelled(x) t/labelled(bump-str(x)))
     ]

Def bump-direction Fn {d: direction} The direction
  If eq(d t/north) t/east
  If eq(d t/east)  t/south
  If eq(d t/south) t/west
  t/north


Export {name: bool-rt iface: "roundtrip:suite/values"}
Def bool-rt Fn {v: bool} The bool not(v)

Export {name: s8-rt iface: "roundtrip:suite/values"}
Def s8-rt Fn {v: s8} The s8 wrap-s8(v)

Export {name: s16-rt iface: "roundtrip:suite/values"}
Def s16-rt Fn {v: s16} The s16 wrap-s16(v)

Export {name: s32-rt iface: "roundtrip:suite/values"}
Def s32-rt Fn {v: s32} The s32 wrap-s32(v)

Export {name: s64-rt iface: "roundtrip:suite/values"}
Def s64-rt Fn {v: s64} The s64 wrap-s64(v)

Export {name: u8-rt iface: "roundtrip:suite/values"}
Def u8-rt Fn {v: u8} The u8 wrap-u8(v)

Export {name: u16-rt iface: "roundtrip:suite/values"}
Def u16-rt Fn {v: u16} The u16 wrap-u16(v)

Export {name: u32-rt iface: "roundtrip:suite/values"}
Def u32-rt Fn {v: u32} The u32 wrap-u32(v)

Export {name: u64-rt iface: "roundtrip:suite/values"}
Def u64-rt Fn {v: u64} The u64 wrap-u64(v)

Export {name: f32-rt iface: "roundtrip:suite/values"}
Def f32-rt Fn {v: f32} The f32 add(v 1.0)

Export {name: f64-rt iface: "roundtrip:suite/values"}
Def f64-rt Fn {v: f64} The f64 add(v 1.0)

Export {name: char-rt iface: "roundtrip:suite/values"}
Def char-rt Fn {v: char} The char next-char(v)

Export {name: string-rt iface: "roundtrip:suite/values"}
Def string-rt Fn {v: string} The string bump-str(v)


Export {name: list-u8-rt iface: "roundtrip:suite/values"}
Def list-u8-rt Fn {v: list(u8)} The list(u8) map(wrap-u8 v)

Export {name: list-string-rt iface: "roundtrip:suite/values"}
Def list-string-rt Fn {v: list(string)} The list(string) map(bump-str v)

Export {name: list-list-u8-rt iface: "roundtrip:suite/values"}
Def list-list-u8-rt Fn {v: list(list(u8))} The list(list(u8))
  map(Fn {xs} map(wrap-u8 xs) v)

Export {name: option-u8-rt iface: "roundtrip:suite/values"}
Def option-u8-rt Fn {v: option(u8)} The option(u8)
  Match v [
    (some(n) some(wrap-u8(n)))
    (none()  none)
  ]

Export {name: option-shape-rt iface: "roundtrip:suite/values"}
Def option-shape-rt Fn {v: option(shape)} The option(shape)
  Match v [
    (some(s) some(bump-shape(s)))
    (none()  none)
  ]

Export {name: result-rt iface: "roundtrip:suite/values"}
Def result-rt Fn {v: result(_ _)} The result(_ _)
  Match v [
    (ok()  err())
    (err() ok())
  ]

Export {name: result-u32-rt iface: "roundtrip:suite/values"}
Def result-u32-rt Fn {v: result(u32 _)} The result(u32 _)
  Match v [
    (ok(n) ok(wrap-u32(n)))
    (err() err())
  ]

Export {name: result-string-err-rt iface: "roundtrip:suite/values"}
Def result-string-err-rt Fn {v: result(_ string)} The result(_ string)
  Match v [
    (ok()   ok())
    (err(s) err(bump-str(s)))
  ]

Export {name: result-u32-string-rt iface: "roundtrip:suite/values"}
Def result-u32-string-rt Fn {v: result(u32 string)} The result(u32 string)
  Match v [
    (ok(n)  ok(wrap-u32(n)))
    (err(s) err(bump-str(s)))
  ]

Export {name: result-tuple-direction-rt iface: "roundtrip:suite/values"}
Def result-tuple-direction-rt Fn {v: result(tuple(u8 u8) direction)}
  The result(tuple(u8 u8) direction)
  Match v [
    (ok(p)  ok(Match p [((a b) tuple2(wrap-u8(a) wrap-u8(b)))]))
    (err(d) err(bump-direction(d)))
  ]

Export {name: tuple-rt iface: "roundtrip:suite/values"}
Def tuple-rt Fn {v: tuple(u8 string bool)} The tuple(u8 string bool)
  Match v [((n s b) tuple3(wrap-u8(n) bump-str(s) not(b)))]

Export {name: tuple-nested-rt iface: "roundtrip:suite/values"}
Def tuple-nested-rt Fn {v: tuple(point list(u8))} The tuple(point list(u8))
  Match v [((p xs) tuple2(bump-point(p) map(wrap-u8 xs)))]


Export {name: point-rt iface: "roundtrip:suite/values"}
Def point-rt Fn {v: point} The point bump-point(v)

Export {name: every-primitive-rt iface: "roundtrip:suite/values"}
Def every-primitive-rt Fn {v: every-primitive} The every-primitive
  Match v [
    ({a: pa b: pb c: pc d: pd e: pe f: pf g: pg h: ph i: pi j: pj k: pk l: pl m: pm}
     {a: not(pa) b: wrap-s8(pb) c: wrap-s16(pc) d: wrap-s32(pd) e: wrap-s64(pe)
      f: wrap-u8(pf) g: wrap-u16(pg) h: wrap-u32(ph) i: wrap-u64(pi)
      j: add(pj 1.0) k: add(pk 1.0) l: next-char(pl) m: bump-str(pm)})
  ]

Export {name: awkward-rt iface: "roundtrip:suite/values"}
Def awkward-rt Fn {v: awkward} The awkward
  Match v [({record: r list: l} {record: wrap-u32(r) list: bump-str(l)})]

Export {name: shape-rt iface: "roundtrip:suite/values"}
Def shape-rt Fn {v: shape} The shape bump-shape(v)

Export {name: direction-rt iface: "roundtrip:suite/values"}
Def direction-rt Fn {v: direction} The direction bump-direction(v)

Export {name: permissions-rt iface: "roundtrip:suite/values"}
Def permissions-rt Fn {v: permissions} The permissions
  Match v [
    ({}                      The permissions {read write exec admin})
    ({read}                  The permissions {write exec admin})
    ({write}                 The permissions {read exec admin})
    ({exec}                  The permissions {read write admin})
    ({admin}                 The permissions {read write exec})
    ({read write}            The permissions {exec admin})
    ({read exec}             The permissions {write admin})
    ({read admin}            The permissions {write exec})
    ({write exec}            The permissions {read admin})
    ({write admin}           The permissions {read exec})
    ({exec admin}            The permissions {read write})
    ({read write exec}       The permissions {admin})
    ({read write admin}      The permissions {exec})
    ({read exec admin}       The permissions {write})
    ({write exec admin}      The permissions {read})
    ({read write exec admin} The permissions {})
  ]

Export {name: points-rt iface: "roundtrip:suite/values"}
Def points-rt Fn {v: points} The points map(bump-point v)


Export {name: no-params iface: "roundtrip:suite/values"}
Def no-params Fn {} The u32 42

Export {name: no-result iface: "roundtrip:suite/values"}
Def no-result Fn {v: u32} drop(v)

Export {name: no-params-no-result iface: "roundtrip:suite/values"}
Def no-params-no-result Fn {} drop(0)

Export {name: multi-param iface: "roundtrip:suite/values"}
Def multi-param Fn {a: u8 b: u16 c: u32 d: u64} The u64
  add(add(add(a b) c) add(d 1))



DefResource counter {
  New: Fn {start: u32} cell-new(start)
  next: Fn {self: counter} The u32
    Let {v: cell-get(self)}
      Do [cell-set(self add(v 1)) v]
  value: Fn {self: counter} The u32 cell-get(self)
  sum: Static Fn {values: list(u32)} The counter counter(sum-list(values))
}
Export {iface: "roundtrip:suite/resources" name: counter}

Def sum-list Fn {vs: list(u32)} The u32
  If eq(vs []) 0 add(head(vs) sum-list(tail(vs)))

Export {name: make-counter iface: "roundtrip:suite/resources"}
Def make-counter Fn {start: u32} The counter counter(start)

Export {name: bump-counter iface: "roundtrip:suite/resources"}
Def bump-counter Fn {c: borrow(counter)}
  The u32 Do [counter/next(c) counter/value(c)]

Export {name: take-counter iface: "roundtrip:suite/resources"}
Def take-counter Fn {c: counter} The u32 counter/value(c)

Export {name: counter-round-trip iface: "roundtrip:suite/resources"}
Def counter-round-trip Fn {c: counter} The counter counter(add(counter/value(c) 1))

Export {name: counter-to-point iface: "roundtrip:suite/resources"}
Def counter-to-point Fn {c: borrow(counter)} The point
  Let {x: to-s32(counter/value(c))}
    {x: x y: add(x 1)}


Import {pkg: "roundtrip:suite/values" as: iv}
Import {pkg: "roundtrip:suite/resources" as: ir}

Def check Fn {name: string pass: bool} The string
  If pass "" name

Def collect Fn {xs: list(string) acc: string} The string
  If eq(xs [])
     acc
     Let {h: head(xs)
          acc2: If eq(h "") acc str-cat(str-cat(acc " ") h)}
       collect(tail(xs) acc2)

Def values-fails Fn {} The string
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

Def resources-fails Fn {} The string
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

Def report Fn {fails: string} The result(_ list(string))
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
