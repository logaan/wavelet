// EXPECT: error
// A range-violating narrowing conversion fails on both engines at the same
// stream position.

{u8: to-u8(255) s8: to-s8(-128) u16: to-u16(65535)}

to-u8(add(255 1))
