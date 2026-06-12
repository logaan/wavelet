# Notes

1. [ ] Where can I run the command line tools?
1. [ ] I should MIT license this.
1. [ ] I need a `readme.md`.
1. [ ] I need a `scripts` directory.
   1. `cargo test`
   1. `cargo run`
   1. `cargo build`
      1. `./target/debug/wavelet`
1. 5,700 lines of Rust.
   1. 34 tests.
   1. What's my code coverage level?
1. [ ] How big is my file?
   1. 4.0kb seems ok, given that it's packing wit data too.
1. [ ] Have a read of the wat representation of the compiled wasm.
1. [ ] It might be nice to be able to write some functions as pure `wait` style
wasm.
1. [ ] It owuld be nice to be able to kcompile to wat directory with fully
maintained $ style names.
1. [ ] Run doesn't really say home to ship a featuer.

## `wavelet` app

1. [ ] `wavelet wit` shows that shout has created an `api` interface.
   1. But that wasn't specified anywhere in the implementation.
   1. If our components are going to have to conform to implement externally
      defined interfaces then shouldn't that perhaps be more well controlled
      and explicit?
1. [ ] `wavelet expand` didn't do anything with the shout example.
   1. I'm guessing because it's using all built in forms. If I had user defined
      macros it probably wouldn't expanded them.
1. `wavelet run` and `wavelet build` feel quite rust inspired.

## Files

### Cargo.toml

1. [ ] What is `wac-graph`?
1. [ ] What is `wasm-encoder`?
1. [ ] What is `wit-component`?
1. [ ] What is `wit-parser`?

### examples

#### main.wvl

1. [ ] Why are we importing `demo:shout/api` rather than `demo/shout` or `demo/shout`?
1. [ ] Having a `shout` function inside a `shout` module isn't great naming.
1. [ ] Is `Target` something that exists in wit?
1. [ ] Where is it getting the definition of `wasi:cli/command` from?
1. [ ] If we weren't targeting `wasi` could we compile to bare components?
1. [ ] `Import` taking the `pkg` as a named argument rather than a positional
   one feels a bit odd. If it's a required argument I'd lean towards positional.
1. [ ] Where did `len`, `args`, and `println` come from?
1. [ ] Is any of my sugar valid Wave syntax? Avoiding that overlap seems helpful.

#### shout.wvl

1. [ ] Where did `str-cat` and `upper` come from?
1. [ ] We need some syntax highlighting.
1. [ ] It'd be nice if we could support `#!/usr/bin/env wavelet`
