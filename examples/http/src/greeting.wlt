Package "http:greeting@0.1.0"

Export greet
Def greet Fn {name: string}
  str-cat("Hello, " name "!")
