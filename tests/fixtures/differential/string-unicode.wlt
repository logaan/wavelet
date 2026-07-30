// EXPECT: ok
// Non-ASCII strings: len counts CHARS (not bytes) on both engines, and
// split / contains / str-cat round-trip multi-byte content intact.
//
// upper/lower are deliberately NOT probed with non-ASCII input here: the
// backend's case mapping is ASCII-only while the oracle uses full Unicode
// case mapping — a known divergence raised as a finding on the step-6 Thing
// (see the lot updates), too large to fix as a fixture side-quest.

len("héllo")

[len("αβγ") len("héllo") len("ascii") len("")]

split("α,β,γ" ",")

[contains("naïve" "ï") contains("naïve" "i")]

str-cat("π = " to-string(3.14159))

to-string("héllo αβγ")
