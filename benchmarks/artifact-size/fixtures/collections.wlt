Package "bench:collections"

Export run
Def run Fn {values: list(s64)}
  join(map(Fn {n: s64} to-string(mul(n n)) values) ",")
