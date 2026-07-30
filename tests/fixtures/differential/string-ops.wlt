// EXPECT: ok
// String builtins: case mapping, concatenation, split/join/contains,
// len/empty, and to-string round-tripping.

str-cat(upper("ada") " " "Lovelace")

[upper("hello") lower("WORLD") upper("") lower("MiXeD-42")]

{split: split("a,b,c" ",") join: join(["x" "y" "z"] "-") contains: contains("hello" "ell")}

split("no-separator-here" ",")

split("" ",")

[contains("" "") contains("abc" "") contains("" "a")]

{len1: len("hello") len2: len("") empty1: empty("") empty2: empty("x")}

str-cat("count = " to-string(42))

to-string("quoted")

join(split("a,b,c" ",") ",")
