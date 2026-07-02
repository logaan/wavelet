//! The two hard-coded seed tables. The `seed-b` cargo feature selects the
//! second one; the build script produces one wasm file per table so a callee
//! cannot hard-code responses. Seed A sticks to unremarkable values, seed B
//! leans on edges: type maxima that must wrap, empty strings and lists,
//! `none`/`err` sides, and supplementary-plane characters.
//!
//! Counter starts stay below 2^31 in both tables so `counter-to-point`'s
//! `value as s32` never has to reinterpret.

#[cfg(not(feature = "seed-b"))]
pub use seed_a::*;
#[cfg(feature = "seed-b")]
pub use seed_b::*;

#[cfg(not(feature = "seed-b"))]
mod seed_a {
    use crate::bindings::roundtrip::suite::types::{
        Awkward, Direction, EveryPrimitive, Permissions, Point, Shape,
    };

    pub const BOOL: bool = true;
    pub const S8: i8 = -5;
    pub const S16: i16 = -300;
    pub const S32: i32 = -70_000;
    pub const S64: i64 = -5_000_000_000;
    pub const U8: u8 = 10;
    pub const U16: u16 = 1_000;
    pub const U32: u32 = 100_000;
    pub const U64: u64 = 10_000_000_000;
    pub const F32: f32 = 1.5;
    pub const F64: f64 = 100.25;
    pub const CHAR: char = 'a';

    pub fn string() -> String {
        "hello".to_string()
    }

    pub fn list_u8() -> Vec<u8> {
        vec![1, 2, 3]
    }

    pub fn list_string() -> Vec<String> {
        vec!["alpha".to_string(), "beta".to_string()]
    }

    pub fn list_list_u8() -> Vec<Vec<u8>> {
        vec![vec![1], vec![2, 3]]
    }

    pub const OPTION_U8: Option<u8> = Some(7);

    pub fn option_shape() -> Option<Shape> {
        Some(Shape::Circle(2.5))
    }

    pub const RESULT_BARE: Result<(), ()> = Ok(());
    pub const RESULT_U32: Result<u32, ()> = Ok(3);

    pub fn result_string_err() -> Result<(), String> {
        Ok(())
    }

    pub fn result_u32_string() -> Result<u32, String> {
        Ok(9)
    }

    pub const RESULT_TUPLE_DIRECTION: Result<(u8, u8), Direction> = Ok((1, 2));

    pub fn tuple() -> (u8, String, bool) {
        (1, "x".to_string(), true)
    }

    pub fn tuple_nested() -> (Point, Vec<u8>) {
        (Point { x: 1, y: 2 }, vec![3])
    }

    pub const POINT: Point = Point { x: 3, y: 4 };

    pub fn every_primitive() -> EveryPrimitive {
        EveryPrimitive {
            a: true,
            b: -1,
            c: -2,
            d: -3,
            e: -4,
            f: 1,
            g: 2,
            h: 3,
            i: 4,
            j: 0.5,
            k: 1.25,
            l: 'm',
            m: "mid".to_string(),
        }
    }

    pub fn awkward() -> Awkward {
        Awkward { record: 1, list: "r".to_string() }
    }

    pub fn shape() -> Shape {
        Shape::Rect(Point { x: 1, y: 2 })
    }

    pub const DIRECTION: Direction = Direction::North;

    pub fn permissions() -> Permissions {
        Permissions::READ
    }

    pub fn points() -> Vec<Point> {
        vec![Point { x: 1, y: 2 }]
    }

    pub const MULTI: (u8, u16, u32, u64) = (1, 2, 3, 4);
    pub const NO_RESULT_ARG: u32 = 5;
    pub const COUNTER_START: u32 = 5;

    pub fn counter_sum_list() -> Vec<u32> {
        vec![1, 2, 3]
    }
}

#[cfg(feature = "seed-b")]
mod seed_b {
    use crate::bindings::roundtrip::suite::types::{
        Awkward, Direction, EveryPrimitive, Permissions, Point, Shape,
    };

    pub const BOOL: bool = false;
    pub const S8: i8 = i8::MAX;
    pub const S16: i16 = i16::MAX;
    pub const S32: i32 = i32::MAX;
    pub const S64: i64 = i64::MAX;
    pub const U8: u8 = u8::MAX;
    pub const U16: u16 = u16::MAX;
    pub const U32: u32 = u32::MAX;
    pub const U64: u64 = u64::MAX;
    pub const F32: f32 = -2.5;
    pub const F64: f64 = -0.75;
    pub const CHAR: char = '🦀';

    pub fn string() -> String {
        String::new()
    }

    pub fn list_u8() -> Vec<u8> {
        Vec::new()
    }

    pub fn list_string() -> Vec<String> {
        vec![String::new(), "z".to_string()]
    }

    pub fn list_list_u8() -> Vec<Vec<u8>> {
        vec![Vec::new()]
    }

    pub const OPTION_U8: Option<u8> = None;

    pub fn option_shape() -> Option<Shape> {
        Some(Shape::Dot)
    }

    pub const RESULT_BARE: Result<(), ()> = Err(());
    pub const RESULT_U32: Result<u32, ()> = Err(());

    pub fn result_string_err() -> Result<(), String> {
        Err("bad".to_string())
    }

    pub fn result_u32_string() -> Result<u32, String> {
        Err("nope".to_string())
    }

    pub const RESULT_TUPLE_DIRECTION: Result<(u8, u8), Direction> =
        Err(Direction::West);

    pub fn tuple() -> (u8, String, bool) {
        (255, String::new(), false)
    }

    pub fn tuple_nested() -> (Point, Vec<u8>) {
        (Point { x: -1, y: -2 }, Vec::new())
    }

    pub const POINT: Point = Point { x: i32::MAX, y: i32::MIN };

    pub fn every_primitive() -> EveryPrimitive {
        EveryPrimitive {
            a: false,
            b: i8::MIN,
            c: i16::MIN,
            d: i32::MIN,
            e: i64::MIN,
            f: u8::MAX,
            g: u16::MAX,
            h: u32::MAX,
            i: u64::MAX,
            j: -0.5,
            k: -2.25,
            l: '\u{10FFFF}',
            m: "末尾".to_string(),
        }
    }

    pub fn awkward() -> Awkward {
        Awkward { record: u32::MAX, list: String::new() }
    }

    pub fn shape() -> Shape {
        Shape::Labelled("hi".to_string())
    }

    pub const DIRECTION: Direction = Direction::West;

    pub fn permissions() -> Permissions {
        Permissions::all()
    }

    pub fn points() -> Vec<Point> {
        Vec::new()
    }

    pub const MULTI: (u8, u16, u32, u64) = (u8::MAX, u16::MAX, u32::MAX, u64::MAX);
    pub const NO_RESULT_ARG: u32 = 500;
    pub const COUNTER_START: u32 = 1_000;

    pub fn counter_sum_list() -> Vec<u32> {
        vec![u32::MAX, 6]
    }
}
