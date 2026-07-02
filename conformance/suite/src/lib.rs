//! The symmetric round-trip artifact.
//!
//! As a callee it exports `values` and `resources`, applying the documented
//! transform to whatever it receives. As a caller it exports `runner`, whose
//! `run` drives every function of the *imported* `values` and `resources`
//! with this build's seed table and reports mismatches.

#[allow(warnings)]
mod bindings;
mod seeds;
mod transform;

use std::cell::Cell;
use std::fmt::Debug;

use bindings::exports::roundtrip::suite::resources::{
    Counter as ExportCounter, CounterBorrow, Guest as ResourcesGuest, GuestCounter,
};
use bindings::exports::roundtrip::suite::runner::Guest as RunnerGuest;
use bindings::exports::roundtrip::suite::values::Guest as ValuesGuest;
use bindings::roundtrip::suite::resources as imported_resources;
use bindings::roundtrip::suite::types::{
    Awkward, Direction, EveryPrimitive, Permissions, Point, Points, Shape,
};
use bindings::roundtrip::suite::values as imported_values;
use transform::*;

struct Component;

// --- callee role: apply the documented transform to whatever arrives ---

impl ValuesGuest for Component {
    fn bool_rt(v: bool) -> bool {
        !v
    }
    fn s8_rt(v: i8) -> i8 {
        v.wrapping_add(1)
    }
    fn s16_rt(v: i16) -> i16 {
        v.wrapping_add(1)
    }
    fn s32_rt(v: i32) -> i32 {
        v.wrapping_add(1)
    }
    fn s64_rt(v: i64) -> i64 {
        v.wrapping_add(1)
    }
    fn u8_rt(v: u8) -> u8 {
        v.wrapping_add(1)
    }
    fn u16_rt(v: u16) -> u16 {
        v.wrapping_add(1)
    }
    fn u32_rt(v: u32) -> u32 {
        v.wrapping_add(1)
    }
    fn u64_rt(v: u64) -> u64 {
        v.wrapping_add(1)
    }
    fn f32_rt(v: f32) -> f32 {
        v + 1.0
    }
    fn f64_rt(v: f64) -> f64 {
        v + 1.0
    }
    fn char_rt(v: char) -> char {
        bump_char(v)
    }
    fn string_rt(v: String) -> String {
        bump_string(&v)
    }

    fn list_u8_rt(v: Vec<u8>) -> Vec<u8> {
        bump_list_u8(&v)
    }
    fn list_string_rt(v: Vec<String>) -> Vec<String> {
        bump_list_string(&v)
    }
    fn list_list_u8_rt(v: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        bump_list_list_u8(&v)
    }
    fn option_u8_rt(v: Option<u8>) -> Option<u8> {
        v.map(|n| n.wrapping_add(1))
    }
    fn option_shape_rt(v: Option<Shape>) -> Option<Shape> {
        v.as_ref().map(bump_shape)
    }
    fn result_rt(v: Result<(), ()>) -> Result<(), ()> {
        flip_result(v)
    }
    fn result_u32_rt(v: Result<u32, ()>) -> Result<u32, ()> {
        v.map(|n| n.wrapping_add(1))
    }
    fn result_string_err_rt(v: Result<(), String>) -> Result<(), String> {
        v.map_err(|e| bump_string(&e))
    }
    fn result_u32_string_rt(v: Result<u32, String>) -> Result<u32, String> {
        v.map(|n| n.wrapping_add(1)).map_err(|e| bump_string(&e))
    }
    fn result_tuple_direction_rt(
        v: Result<(u8, u8), Direction>,
    ) -> Result<(u8, u8), Direction> {
        v.map(|(a, b)| (a.wrapping_add(1), b.wrapping_add(1)))
            .map_err(bump_direction)
    }
    fn tuple_rt(v: (u8, String, bool)) -> (u8, String, bool) {
        (v.0.wrapping_add(1), bump_string(&v.1), !v.2)
    }
    fn tuple_nested_rt(v: (Point, Vec<u8>)) -> (Point, Vec<u8>) {
        (bump_point(v.0), bump_list_u8(&v.1))
    }

    fn point_rt(v: Point) -> Point {
        bump_point(v)
    }
    fn every_primitive_rt(v: EveryPrimitive) -> EveryPrimitive {
        bump_every_primitive(&v)
    }
    fn awkward_rt(v: Awkward) -> Awkward {
        bump_awkward(&v)
    }
    fn shape_rt(v: Shape) -> Shape {
        bump_shape(&v)
    }
    fn direction_rt(v: Direction) -> Direction {
        bump_direction(v)
    }
    fn permissions_rt(v: Permissions) -> Permissions {
        bump_permissions(v)
    }
    fn points_rt(v: Points) -> Points {
        bump_points(&v)
    }

    fn no_params() -> u32 {
        42
    }
    fn no_result(_v: u32) {}
    fn no_params_no_result() {}
    fn multi_param(a: u8, b: u16, c: u32, d: u64) -> u64 {
        transform::multi_param(a, b, c, d)
    }
}

struct MyCounter(Cell<u32>);

impl GuestCounter for MyCounter {
    fn new(start: u32) -> Self {
        MyCounter(Cell::new(start))
    }
    fn next(&self) -> u32 {
        let v = self.0.get();
        self.0.set(v.wrapping_add(1));
        v
    }
    fn value(&self) -> u32 {
        self.0.get()
    }
    fn sum(values: Vec<u32>) -> ExportCounter {
        ExportCounter::new(MyCounter::new(wrapping_sum(&values)))
    }
}

impl ResourcesGuest for Component {
    type Counter = MyCounter;

    fn make_counter(start: u32) -> ExportCounter {
        ExportCounter::new(MyCounter::new(start))
    }
    fn bump_counter(c: CounterBorrow<'_>) -> u32 {
        let c = c.get::<MyCounter>();
        c.0.set(c.0.get().wrapping_add(1));
        c.0.get()
    }
    fn take_counter(c: ExportCounter) -> u32 {
        c.get::<MyCounter>().value()
    }
    fn counter_round_trip(c: ExportCounter) -> ExportCounter {
        let next = c.get::<MyCounter>().value().wrapping_add(1);
        ExportCounter::new(MyCounter::new(next))
    }
    fn counter_to_point(c: CounterBorrow<'_>) -> Point {
        let v = c.get::<MyCounter>().value() as i32;
        Point { x: v, y: v.wrapping_add(1) }
    }
}

// --- caller role: drive the imported suite with this build's seeds ---

fn check<T: PartialEq + Debug>(fails: &mut Vec<String>, name: &str, want: T, got: T) {
    if want != got {
        fails.push(format!("{name}: want {want:?}, got {got:?}"));
    }
}

fn run_values(fails: &mut Vec<String>) {
    use imported_values as iv;

    check(fails, "bool-rt", !seeds::BOOL, iv::bool_rt(seeds::BOOL));
    check(fails, "s8-rt", seeds::S8.wrapping_add(1), iv::s8_rt(seeds::S8));
    check(fails, "s16-rt", seeds::S16.wrapping_add(1), iv::s16_rt(seeds::S16));
    check(fails, "s32-rt", seeds::S32.wrapping_add(1), iv::s32_rt(seeds::S32));
    check(fails, "s64-rt", seeds::S64.wrapping_add(1), iv::s64_rt(seeds::S64));
    check(fails, "u8-rt", seeds::U8.wrapping_add(1), iv::u8_rt(seeds::U8));
    check(fails, "u16-rt", seeds::U16.wrapping_add(1), iv::u16_rt(seeds::U16));
    check(fails, "u32-rt", seeds::U32.wrapping_add(1), iv::u32_rt(seeds::U32));
    check(fails, "u64-rt", seeds::U64.wrapping_add(1), iv::u64_rt(seeds::U64));
    check(fails, "f32-rt", seeds::F32 + 1.0, iv::f32_rt(seeds::F32));
    check(fails, "f64-rt", seeds::F64 + 1.0, iv::f64_rt(seeds::F64));
    check(fails, "char-rt", bump_char(seeds::CHAR), iv::char_rt(seeds::CHAR));
    check(
        fails,
        "string-rt",
        bump_string(&seeds::string()),
        iv::string_rt(&seeds::string()),
    );

    check(
        fails,
        "list-u8-rt",
        bump_list_u8(&seeds::list_u8()),
        iv::list_u8_rt(&seeds::list_u8()),
    );
    check(
        fails,
        "list-string-rt",
        bump_list_string(&seeds::list_string()),
        iv::list_string_rt(&seeds::list_string()),
    );
    check(
        fails,
        "list-list-u8-rt",
        bump_list_list_u8(&seeds::list_list_u8()),
        iv::list_list_u8_rt(&seeds::list_list_u8()),
    );
    check(
        fails,
        "option-u8-rt",
        seeds::OPTION_U8.map(|n| n.wrapping_add(1)),
        iv::option_u8_rt(seeds::OPTION_U8),
    );
    check(
        fails,
        "option-shape-rt",
        seeds::option_shape().as_ref().map(bump_shape),
        iv::option_shape_rt(seeds::option_shape().as_ref()),
    );
    check(
        fails,
        "result-rt",
        flip_result(seeds::RESULT_BARE),
        iv::result_rt(seeds::RESULT_BARE),
    );
    check(
        fails,
        "result-u32-rt",
        seeds::RESULT_U32.map(|n| n.wrapping_add(1)),
        iv::result_u32_rt(seeds::RESULT_U32),
    );
    check(
        fails,
        "result-string-err-rt",
        seeds::result_string_err().map_err(|e| bump_string(&e)),
        iv::result_string_err_rt(match &seeds::result_string_err() {
            Ok(()) => Ok(()),
            Err(e) => Err(e),
        }),
    );
    check(
        fails,
        "result-u32-string-rt",
        seeds::result_u32_string()
            .map(|n| n.wrapping_add(1))
            .map_err(|e| bump_string(&e)),
        iv::result_u32_string_rt(match &seeds::result_u32_string() {
            Ok(n) => Ok(*n),
            Err(e) => Err(e),
        }),
    );
    check(
        fails,
        "result-tuple-direction-rt",
        seeds::RESULT_TUPLE_DIRECTION
            .map(|(a, b)| (a.wrapping_add(1), b.wrapping_add(1)))
            .map_err(bump_direction),
        iv::result_tuple_direction_rt(seeds::RESULT_TUPLE_DIRECTION),
    );
    let (t0, t1, t2) = seeds::tuple();
    check(
        fails,
        "tuple-rt",
        (t0.wrapping_add(1), bump_string(&t1), !t2),
        iv::tuple_rt((t0, &t1, t2)),
    );
    let (n0, n1) = seeds::tuple_nested();
    check(
        fails,
        "tuple-nested-rt",
        (bump_point(n0), bump_list_u8(&n1)),
        iv::tuple_nested_rt((&n0, &n1)),
    );

    check(fails, "point-rt", bump_point(seeds::POINT), iv::point_rt(seeds::POINT));
    check(
        fails,
        "every-primitive-rt",
        bump_every_primitive(&seeds::every_primitive()),
        iv::every_primitive_rt(&seeds::every_primitive()),
    );
    check(
        fails,
        "awkward-rt",
        bump_awkward(&seeds::awkward()),
        iv::awkward_rt(&seeds::awkward()),
    );
    check(fails, "shape-rt", bump_shape(&seeds::shape()), iv::shape_rt(&seeds::shape()));
    check(
        fails,
        "direction-rt",
        bump_direction(seeds::DIRECTION),
        iv::direction_rt(seeds::DIRECTION),
    );
    check(
        fails,
        "permissions-rt",
        bump_permissions(seeds::permissions()),
        iv::permissions_rt(seeds::permissions()),
    );
    check(fails, "points-rt", bump_points(&seeds::points()), iv::points_rt(&seeds::points()));

    check(fails, "no-params", 42, iv::no_params());
    iv::no_result(seeds::NO_RESULT_ARG);
    iv::no_params_no_result();
    let (a, b, c, d) = seeds::MULTI;
    check(
        fails,
        "multi-param",
        transform::multi_param(a, b, c, d),
        iv::multi_param(a, b, c, d),
    );
}

fn run_resources(fails: &mut Vec<String>) {
    use imported_resources as ir;

    let start = seeds::COUNTER_START;

    let c = ir::Counter::new(start);
    check(fails, "counter.constructor + counter.next (1st)", start, c.next());
    check(fails, "counter.next (2nd)", start.wrapping_add(1), c.next());
    check(fails, "counter.value", start.wrapping_add(2), c.value());

    let sum_list = seeds::counter_sum_list();
    check(
        fails,
        "counter.sum",
        wrapping_sum(&sum_list),
        ir::Counter::sum(&sum_list).value(),
    );

    check(fails, "make-counter", start, ir::make_counter(start).value());

    let b = ir::Counter::new(start);
    check(fails, "bump-counter", start.wrapping_add(1), ir::bump_counter(&b));
    check(fails, "bump-counter (state)", start.wrapping_add(1), b.value());

    check(fails, "take-counter", start, ir::take_counter(ir::Counter::new(start)));

    check(
        fails,
        "counter-round-trip",
        start.wrapping_add(1),
        ir::counter_round_trip(ir::Counter::new(start)).value(),
    );

    let p = ir::Counter::new(start);
    check(
        fails,
        "counter-to-point",
        Point { x: start as i32, y: (start as i32).wrapping_add(1) },
        ir::counter_to_point(&p),
    );
}

fn finish(fails: Vec<String>) -> Result<(), Vec<String>> {
    if fails.is_empty() { Ok(()) } else { Err(fails) }
}

impl RunnerGuest for Component {
    fn run() -> Result<(), Vec<String>> {
        let mut fails = Vec::new();
        run_values(&mut fails);
        run_resources(&mut fails);
        finish(fails)
    }
    fn run_values() -> Result<(), Vec<String>> {
        let mut fails = Vec::new();
        self::run_values(&mut fails);
        finish(fails)
    }
    fn run_resources() -> Result<(), Vec<String>> {
        let mut fails = Vec::new();
        self::run_resources(&mut fails);
        finish(fails)
    }
}

bindings::export!(Component with_types_in bindings);
