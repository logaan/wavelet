Package "bench:wasi-hello"

Import {pkg: "wasi:cli/stdout@0.2.0" as: stdout}
Import {pkg: "wasi:io/streams@0.2.0" as: streams}

Export {iface: "wasi:cli/run@0.2.0" name: run result: result}
Def run Fn {}
  Let {out: stdout/get-stdout()}
    Do [streams/blocking-write-and-flush(out "Hello, world!\n")
        streams/drop-output-stream(out)
        ok(0)]
