# roundtrip:suite — a WIT round-trip conformance suite

This package exercises every type and feature covered by the stable WIT value and resource suite. Async futures, streams,
and proposal features are outside this suite.

## The protocol

An implementation under test exports the `values` and `resources`
interfaces. For every `*-rt` function the rule is the same: take the
argument, apply the *documented transform* for its type (see each contract below), and return the result. A test driver imports the same
interfaces, calls every function with a hard-coded seed value, and checks
the response against the transform it computed locally. Seeds are private
to each driver build (two builds with different seed tables are shipped),
so a callee cannot hard-code responses; it must actually decompose and
rebuild each value.

## Transform conventions

- integers: add 1, wrapping at the type's width
- floats: add 1.0
- bool: negate
- char: next Unicode scalar value (skipping the surrogate gap,
U+10FFFF wraps to U+0000)
- string: append "!"
- list: transform each element (length preserved)
- option: none stays none; some(v) becomes some(transform(v))
- result with payloads: keep the side, transform the payload
- bare result: flip ok <-> err
- tuple / record: transform each field
- enum: rotate to the next case (last wraps to first)
- flags: toggle every defined member
- variant: per-case rules given on the type
- resources: semantics documented per function on the `resources`
interface

### `interface types`

Type definitions shared by the other interfaces via `use`.

### `record point`

Transform: x and y each gain 1.

### `record every-primitive`

One field per primitive type. Transform: each field follows its
primitive's rule (see package docs).

### `record awkward`

Field names that collide with WIT keywords, spelled with the `%`
escape. Transform: fieldwise (integer +1, string appends "!").

### `variant shape`

Transform, per case:
- dot            -> circle(1.0)
- circle(r)      -> circle(r + 1.0)
- rect(p)        -> rect(transform(p))
- labelled(s)    -> labelled(s + "!")

### `enum direction`

Transform: rotate north -> east -> south -> west -> north.

### `flags permissions`

Transform: toggle every member.

### `type points = list<point>`

A type alias over a compound type. Transform: as list<point>.

### `interface values`

One round-trip function per plain data type and function shape.

### `result-rt: func(v: result) -> result`

Bare result: ok flips to err and err flips to ok.

### `no-params: func() -> u32`

Takes nothing, must return exactly 42.

### `no-result: func(v: u32)`

Returns nothing; must simply not trap.

### `no-params-no-result: func()`

Takes and returns nothing; must simply not trap.

### `multi-param: func(a: u8, b: u16, c: u32, d: u64) -> u64`

Transform: a + b + c + d + 1, all as u64, wrapping.

### `interface resources`

Resource round-trips: handles crossing the boundary in both directions,
constructors, methods, statics, and borrows.

### `resource counter`

A stateful counter.

### `constructor(start: u32)`

Starts the counter at `start`.

### `next: func() -> u32`

Post-increment: returns the current value, then advances by 1.

### `value: func() -> u32`

Returns the current value without advancing.

### `sum: static func(values: list<u32>) -> counter`

Static constructor: a counter whose value is the sum of `values`
(wrapping), starting at 0 for an empty list.

### `make-counter: func(start: u32) -> counter`

Returns a freshly constructed counter starting at `start`
(an owned handle crossing the boundary outward).

### `bump-counter: func(c: borrow<counter>) -> u32`

Advances the borrowed counter once and returns the post-advance value.

### `take-counter: func(c: counter) -> u32`

Consumes the counter and returns its final value.

### `counter-round-trip: func(c: counter) -> counter`

Consumes the counter and returns a new one starting at value + 1.

### `counter-to-point: func(c: borrow<counter>) -> point`

Reads the borrowed counter into a point: x = value as s32,
y = x + 1. (Exercises `use`d types alongside resources.)

### `interface runner`

The test driver's entry point.

### `run: func() -> result<_, list<string>>`

Calls every function of the imported `values` and `resources`
interfaces with this build's seed values and checks each response.
Returns ok on a clean pass, or err with one message per failed check.

### `run-values: func() -> result<_, list<string>>`

The `values` checks alone — so an implementation whose resource
support is missing (or stubbed) still gets a granular data-type report.

### `run-resources: func() -> result<_, list<string>>`

The `resources` checks alone.

### `world stub`

Exports-only world: the terminator at the end of a composition chain
(its exports are never actually called), or a pure callee.

### `world roundtrip`

The symmetric artifact: tests whatever it imports, and can itself be
tested through what it exports. (`include stub` pulls in the exports.)
