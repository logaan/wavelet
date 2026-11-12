export const compiledCases = [
  ['scalar-length', 'len("☃😀aé")', '4'],
  ['character-concatenation', 'str-cat("a" \'☃\' \'😀\')', '"a☃😀"'],
  ['tuple-index', 'get(tuple2(1 "a") 1)', '"a"'],
  ['tuple-length', 'len(tuple2(1 "a"))', '2'],
  ['builtin-fold', 'fold(add 0 [1 2 3 4])', '10'],
  ['builtin-map', 'map(neg [1 2 3])', '[-1, -2, -3]'],
  ['keyword-resource', 'DefResource map { New: Fn {start: u32} cell-new(start) value: Fn {self: map} The u32 cell-get(self) }\nLet {item: map(42)} map/value(item)', '42'],
].map(([name, code, value]) => [name, {code, value, output: ''}]);
