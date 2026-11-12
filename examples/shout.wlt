Package "demo:shout@0.1.0"

Export shout
Def shout Fn {phrase: string}
  str-cat(phrase "!")
