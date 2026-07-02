//! The exports-only terminator for composition chains.
//!
//! A symmetric round-trip artifact both imports and exports the suite, so a
//! composed caller/callee pair still has the callee's imports dangling. This
//! component satisfies them. When the callee is only ever exercised *as* a
//! callee its own imports are never called, so every function here traps.

#[allow(warnings)]
mod bindings;

use bindings::exports::roundtrip::suite::resources::{
    Counter, CounterBorrow, Guest as ResourcesGuest, GuestCounter,
};
use bindings::exports::roundtrip::suite::values::Guest as ValuesGuest;
use bindings::roundtrip::suite::types::{
    Awkward, Direction, EveryPrimitive, Permissions, Point, Points, Shape,
};

struct Stub;

fn never<T>() -> T {
    unreachable!("the stub terminator must never be called")
}

impl ValuesGuest for Stub {
    fn bool_rt(_: bool) -> bool {
        never()
    }
    fn s8_rt(_: i8) -> i8 {
        never()
    }
    fn s16_rt(_: i16) -> i16 {
        never()
    }
    fn s32_rt(_: i32) -> i32 {
        never()
    }
    fn s64_rt(_: i64) -> i64 {
        never()
    }
    fn u8_rt(_: u8) -> u8 {
        never()
    }
    fn u16_rt(_: u16) -> u16 {
        never()
    }
    fn u32_rt(_: u32) -> u32 {
        never()
    }
    fn u64_rt(_: u64) -> u64 {
        never()
    }
    fn f32_rt(_: f32) -> f32 {
        never()
    }
    fn f64_rt(_: f64) -> f64 {
        never()
    }
    fn char_rt(_: char) -> char {
        never()
    }
    fn string_rt(_: String) -> String {
        never()
    }
    fn list_u8_rt(_: Vec<u8>) -> Vec<u8> {
        never()
    }
    fn list_string_rt(_: Vec<String>) -> Vec<String> {
        never()
    }
    fn list_list_u8_rt(_: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        never()
    }
    fn option_u8_rt(_: Option<u8>) -> Option<u8> {
        never()
    }
    fn option_shape_rt(_: Option<Shape>) -> Option<Shape> {
        never()
    }
    fn result_rt(_: Result<(), ()>) -> Result<(), ()> {
        never()
    }
    fn result_u32_rt(_: Result<u32, ()>) -> Result<u32, ()> {
        never()
    }
    fn result_string_err_rt(_: Result<(), String>) -> Result<(), String> {
        never()
    }
    fn result_u32_string_rt(_: Result<u32, String>) -> Result<u32, String> {
        never()
    }
    fn result_tuple_direction_rt(
        _: Result<(u8, u8), Direction>,
    ) -> Result<(u8, u8), Direction> {
        never()
    }
    fn tuple_rt(_: (u8, String, bool)) -> (u8, String, bool) {
        never()
    }
    fn tuple_nested_rt(_: (Point, Vec<u8>)) -> (Point, Vec<u8>) {
        never()
    }
    fn point_rt(_: Point) -> Point {
        never()
    }
    fn every_primitive_rt(_: EveryPrimitive) -> EveryPrimitive {
        never()
    }
    fn awkward_rt(_: Awkward) -> Awkward {
        never()
    }
    fn shape_rt(_: Shape) -> Shape {
        never()
    }
    fn direction_rt(_: Direction) -> Direction {
        never()
    }
    fn permissions_rt(_: Permissions) -> Permissions {
        never()
    }
    fn points_rt(_: Points) -> Points {
        never()
    }
    fn no_params() -> u32 {
        never()
    }
    fn no_result(_: u32) {
        never()
    }
    fn no_params_no_result() {
        never()
    }
    fn multi_param(_: u8, _: u16, _: u32, _: u64) -> u64 {
        never()
    }
}

struct StubCounter;

impl GuestCounter for StubCounter {
    fn new(_: u32) -> Self {
        never()
    }
    fn next(&self) -> u32 {
        never()
    }
    fn value(&self) -> u32 {
        never()
    }
    fn sum(_: Vec<u32>) -> Counter {
        never()
    }
}

impl ResourcesGuest for Stub {
    type Counter = StubCounter;

    fn make_counter(_: u32) -> Counter {
        never()
    }
    fn bump_counter(_: CounterBorrow<'_>) -> u32 {
        never()
    }
    fn take_counter(_: Counter) -> u32 {
        never()
    }
    fn counter_round_trip(_: Counter) -> Counter {
        never()
    }
    fn counter_to_point(_: CounterBorrow<'_>) -> Point {
        never()
    }
}

bindings::export!(Stub with_types_in bindings);
