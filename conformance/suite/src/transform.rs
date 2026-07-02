//! The documented transforms, shared by the exported implementation (the
//! callee role) and the runner's expected values (the caller role). The WIT
//! package docs in `wit/world.wit` are the normative description; this module
//! is their Rust rendering.

use crate::bindings::roundtrip::suite::types::{
    Awkward, Direction, EveryPrimitive, Permissions, Point, Shape,
};

pub fn bump_char(v: char) -> char {
    let mut u = v as u32 + 1;
    if (0xD800..=0xDFFF).contains(&u) {
        u = 0xE000;
    }
    if u > 0x10FFFF {
        u = 0;
    }
    char::from_u32(u).unwrap()
}

pub fn bump_string(v: &str) -> String {
    format!("{v}!")
}

pub fn bump_list_u8(v: &[u8]) -> Vec<u8> {
    v.iter().map(|n| n.wrapping_add(1)).collect()
}

pub fn bump_list_string(v: &[String]) -> Vec<String> {
    v.iter().map(|s| bump_string(s)).collect()
}

pub fn bump_list_list_u8(v: &[Vec<u8>]) -> Vec<Vec<u8>> {
    v.iter().map(|inner| bump_list_u8(inner)).collect()
}

pub fn flip_result(v: Result<(), ()>) -> Result<(), ()> {
    match v {
        Ok(()) => Err(()),
        Err(()) => Ok(()),
    }
}

pub fn bump_point(v: Point) -> Point {
    Point { x: v.x.wrapping_add(1), y: v.y.wrapping_add(1) }
}

pub fn bump_every_primitive(v: &EveryPrimitive) -> EveryPrimitive {
    EveryPrimitive {
        a: !v.a,
        b: v.b.wrapping_add(1),
        c: v.c.wrapping_add(1),
        d: v.d.wrapping_add(1),
        e: v.e.wrapping_add(1),
        f: v.f.wrapping_add(1),
        g: v.g.wrapping_add(1),
        h: v.h.wrapping_add(1),
        i: v.i.wrapping_add(1),
        j: v.j + 1.0,
        k: v.k + 1.0,
        l: bump_char(v.l),
        m: bump_string(&v.m),
    }
}

pub fn bump_awkward(v: &Awkward) -> Awkward {
    Awkward { record: v.record.wrapping_add(1), list: bump_string(&v.list) }
}

pub fn bump_shape(v: &Shape) -> Shape {
    match v {
        Shape::Dot => Shape::Circle(1.0),
        Shape::Circle(r) => Shape::Circle(r + 1.0),
        Shape::Rect(p) => Shape::Rect(bump_point(*p)),
        Shape::Labelled(s) => Shape::Labelled(bump_string(s)),
    }
}

pub fn bump_direction(v: Direction) -> Direction {
    match v {
        Direction::North => Direction::East,
        Direction::East => Direction::South,
        Direction::South => Direction::West,
        Direction::West => Direction::North,
    }
}

pub fn bump_permissions(v: Permissions) -> Permissions {
    v ^ Permissions::all()
}

pub fn bump_points(v: &[Point]) -> Vec<Point> {
    v.iter().map(|p| bump_point(*p)).collect()
}

pub fn wrapping_sum(values: &[u32]) -> u32 {
    values.iter().fold(0u32, |acc, n| acc.wrapping_add(*n))
}

pub fn multi_param(a: u8, b: u16, c: u32, d: u64) -> u64 {
    (a as u64)
        .wrapping_add(b as u64)
        .wrapping_add(c as u64)
        .wrapping_add(d)
        .wrapping_add(1)
}
