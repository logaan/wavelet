# Notes

## 15 June

1. [ ] Allow for alternatives to the standard library.
   1. You could have an alternative version of `Package` that doesn't implicitly import the standard library.
   1. Or maybe just don't import it by default ever.
   1. Maybe it doesn't even need to be bundled with the language. It's just another dependency that you pull down.
   1. I can provide a basic one that gives you all of the basic functions.
   1. But if someone else wants to do a purely functional version, or a real time version, or something like that then your file can just as easily live in that world.
   1. Does this become a problem for macros? Probably not once they're componentised.
   1. Once macros are componentised do we even need macros like `Quote`, `Quasi` and `Unquote` in the core language?
   1. For that matter do we even need `If` or `Match`?
   1. Yes. Because they aren't macros they're special forms. They couldn't be implemented as macros.
1. [ ] Maybe drop the interpreter?
1. [ ] I'd like a docs page that explicitly enumerates all of the sugar.
   1. I'm not sure that we need "Clojure-style: `kv/open`, `kv/get`" as shown [in the docs].
   1. We could do automatic renaming? So people write `kv-open`.
   1. It would mean one less form of sugar.
1. [ ] Make the compiler a bit more modular
   1. If it's just going off the AST then people can potentially put whatever sugar in front that they want.
   1. And because of our component boundaries you could even have language authors who're writing with a very different syntax to you.
   1. I could like a Clojure style lisp syntax. You could like a ML style syntax like Grain.
   1. Having 2 steps in the compilation pipeline is a small cost to pay.
   1. Potentially it could make sense to go Wavelet -> Wave (ast) -> Wat -> Wasm components -> Composed component -> Component with bundled runtime.
   1. With each step represented with files in your build directory.
   1. Though given the rust libraries I'm not sure that it would simplify things at all to target Wat rather than going straight to the wasm component.
1. [ ] Add a test that uses the wave parser to validate homoiconicity.
   1. Reads in an example app that exercises every syntax sugar feature and every language feature.
   1. Then writes that read data out to a file.
   1. Wave parses it, if wave can't then the test fails.
   1. Wave writes out to a new file and the two should be the same.
   1. Then the wave written file is taken through the remaining compiler steps.
1. [ ] Switch to tuples for calls.
1. [ ] Maybe compile closures to components so they can be sent over the wire?
   1. Like Erlang.
1. [ ] Maybe make the compiler itself a component that you can chose to to import.
   1. Enabling the closure compilation above?
   1. It might be easier to do it using explicit partials.

[in the docs]: https://logaan.github.io/wavelet/components#imports

## From todo.md

- [ ] Qualified TitleCase macros `Dsl/Element` arity reading (parses, but arity
      lookup ignores the alias; revisit with macro imports in Phase 2)
- [ ] Macro components: instantiate wasm at compile time, `manifest`/`expand`
      interface, `Import {… macros: true}`
- [ ] Resource handles beyond `cell`; owned-handle drop semantics
- [ ] Boundary coercions + `safely` wrapper semantics
- [ ] Richer inference for lists/options/results — currently errors and asks
      for annotations when it cannot infer
- [ ] v0 backend gaps still open: option/result *params* with mismatched arm
      flat shapes (needs the numeric-widening variant join); >16-flat param
      spill-to-memory; named 3+-case `variant`/`enum` DefTypes across boundaries
      (blocked anyway — the dynamic core has no constructor for user variant
      cases, only the `ok`/`some`/`err`/`none` builtins); GC (leaks by design),
      `compose.wave` manifest, `--fuse`
- [ ] Closures across boundaries → resource lifting
- [ ] Registry fetch `wavelet add`
- [ ] Exhaustiveness lint
- [ ] Hygiene

## Previously

1. [ ] Add a readme to the scripts directory.
1. [ ] Edit down the main repo readme.
1. [ ] Prune comments through the code and config files
1. [ ] Test the vscode tooling
1. [ ] Prune back the implementation
1. [ ] Add support for deploying to:
    1. [ ] A stand alone CLI tool. By bundling a runtime.
    1. [ ] JS in the browser
    1. [ ] JS with node / bun
    1. [ ] The JVM?
    1. [ ] Docker?
    1. [ ] Kubernetes?
1. [ ] Have a wasm version of the compiler and/or interpreter published as a package to some kind of wasm package repository?
1. [ ] Add static analysis tools
1. [ ] Have a read of the wat representation of the compiled wasm.
1. [ ] I'd like the file extension to be .wlt rather than wvl
   - Assuming that doesn't conflict with anything else.

## Docs

1. [ ] Should be written for someone unfamiliar with wasm.
1. [ ] Maybe we should drop the interpreter?
   1. If the compiler compiles to a wasm component then the compiler itself can called from the examples in the docs.
   1. It could be part of the standard library.
      1. The composer just leaves it out if no one's using it?
1. [ ] Is the standard library composed in? Or is it part of the compile for each component?
1. [ ] The "argument" language is too strong.
1. [ ] The "NO-FFI!!" example is too strong.
1. [ ] How slow is the compilation?
1. [ ] How will `--fuse` work? Is there much overhead if we don't use it?
1. [ ] Would be good to show examples of building all of the wasi app types
   1. cli
   1. http
   1. deploy to <https://www.fermyon.dev/>
   1. deploy to <https://wasmcloud.com/docs/>
   1. embedding it in a python program
1. [ ] Using each of [the wasi things](https://github.com/WebAssembly/WASI/blob/main/docs/Proposals.md#phase-3---implementation-phase-cg--wg)
   1. clocks
   1. random
   1. filesystem
   1. sockets
1. [ ] Using [wkg](https://component-model.bytecodealliance.org/composing-and-distributing/distributing.html) as a package manager
   1. How to publish a wavelet library to wkg
1. [ ] Need some kind of path and component locating thing?
   1. Having to specify all the individual file names in the examples doesn't look great.
1. [ ] Examples shouldn't be using `foo(1)` syntax for method calls
   1. I think I saw it somewhere. Perhaps it was actually creating a variant.
1. [ ] I don't love the font.
1. [ ] Need some kind of canonical formatting. Ideally with a way for user defined macros to specify it also.
   1. Eg: `If` breaks onto 3 lines

      ```wavelet
      If eq[foo 42]
        ok("Match")
        err("No good")
      ```

   1. Whereas package is probably all on one line.
1. [ ] I think that we maybe don't need to support `f(x y) -> f((x, y))`
    1. `f[x y]` already gives us a way to make positional calls
    1. `f((x))` when you want to make a call with a single positional value, but `f(x y)` when you want to do it with 2 feels like a weird inconsistency.
1. [ ] Why do we need grouping? `(a) -> a` isn't what I'd expect. I would think it should be `(a)`
1. [ ] The wavelet docs shouldn't assume people are familiar with wave.
    1. Having a summary of the types is good.
    1. But we also need a full page with all of the rules for each type.
    1. Eg: What values can be keys in records?
    1. Eg: Wtf are flags?
1. [ ] Macros should use `if-MACRO([c t e])` not tuples.
1. [ ] asdf
1. [ ] `wavelet read` shouldn't need to take `/dev/stdin` as an argument
1. [ ] `Quote` isn't just an "Inside the playground" feature.
1. [ ] Formal grammar specification should be an appendix, not early in the learning flow.
1. [ ] The sugar cases should have their own page and be numbered.
1. [ ] Macros should have their own page with examples of how to write them and why they're so valuable.
1. [ ] Macros shouldn't have special `DefMacro -> def-macro-MACRO` expansion.
    1. It should just be `Def-macro -> def-macro-MACRO`
    1. The only deviation we're making is if your symbol has the first letter capitalised and the rest of the first word lowercase then it's a macro identifier.
1. [ ] "when the last argument is itself a macro" shouldn't it matter regardless of whether it's the last argument or not?
1. Grammar
    1. [x] What's an `atom`?
        1. bool, int, float, char, string
        1. Is `atom` the right term?
           1. For everything except string we could use `scalar`
    1. [ ] What's a `qname`?
        1. Qualified name (with `/`)
    1. [ ] Can we skip the `:` in records?
       1. I don't think so because otherwise they'd look like flags.
    1. This grammar definition doesn't feel like it's totally correct.
        1. Like shouldn't comments be defined?

### Values & Types

1. [ ] Is tuples only working for 2+ types a wave thing?
1. [ ] Can wavelet define resources?
1. [ ] Can wavelet define everything that can be defined in wit?
1. [ ] We communicate the mapping between wavelet and wave, we should do the same for wavelet -> wit
1. [ ] Omg the reason we needed tuples for calling is that it lets us have hetrogenous values.
1. [ ] It seems like a bad idea to allow hetrogenous types inside lists.
1. [ ] Maybe drop support for the flat shorthand?
   1. I guess it seems convenient, but is it a little error prone?
   1. Like there's potential for refactoring to lead to surprises?
1. [ ] Can we drop the unit value?
   1. I like Erlang's convention of just saying "ok"
   1. It's like "nothing went wrong"

### Evaluation

1. [ ] It'd be good to have an "Trivia" callout that beginners can ignore.
   1. Use it for the "Lisp-1" mention.
1. [x] I haven't seen any word on functions being first class yet.
    1. They're there `Fn {f x} (f(f(x))`
1. [ ] Can you call a returned function like `foo[x, y][z]`
1. [ ] Could we implement lambdas as their own component?
    - Or is that nuts?
    - It's probably nuts.
    - It would mean you could send it over the wire though.
    - Like bundle up the captured environment in the component, and have an `apply` method.
1. [ ] Would it be actually a lot easier to understand if we just used tuples for fn calls?
   1. `foo(x y z) -> (foo x y z) -> call(foo, x, y, z)`

### The seventeen special forms

1. [ ] Can we flatten the package level forms into `Package`?
   1. `Target`
   1. `Import`
   1. `Export`
   1. `DefType` maybe?
   1. Maybe just have them as a record argument to `Package`?
   1. Maybe it's a bad idea because things like imports and exports can get kinda complex.
2. [ ] Do we need both `If` and `Match`?
   1. Can't one of them be defined in the standard library using the other
3. [ ] These should probably be presented in groups. At least 3, one for macros, one for package, and the rest.
4. [ ] What is `The`?
5. [ ] We need a logo.

#### Fn

1. [ ] Can functions defined with `Def` at the top level be referenced as values? Even in compiled code?
1. [ ] Can you have a mix of typed and untyped parameters?
1. [ ] Is there any amount of static type checking?

#### If

1. No truthiness is kinda nice.

#### Let

1. [ ] Records are unordered in wave (and wasm?)
   1. [ ] I think we should probably be using an ordered collection
   1. [ ] Then again if we think about lists as potentially changing to being homogeneous that'd only leave tuples.
          Which runs there risk of being a list with too many parenthesis.

#### Do

1. [ ] Can't this be in the standard library?

#### Match

1. [ ] Should this live in the standard library?
1. [ ] Is this doing any clever re-writing for optimisation?
1. [ ] Is there a way to head and tail a list?

#### Quote

1. [ ] If we have `Quasi` then do we need `Quote`?
    1. Couldn't `Quote` just be aware of `Splice` and `Unquote`?
    1. Does that then make it impossible to represent them or something?

#### Defmacro

1. [ ] Could this just be `Macro`?

#### `The`

1. [ ] Could this just be a function?

#### `Deftype`

1. [ ] Could this just be `Type`?

#### `Packge`

1. [ ] Should fully qualified names be first class types?
    1. [ ] Are they not expressible in wave?
1. [ ] Is Package the right name?
    1. Could it be `Module`, `Component`, or `World`?

#### `Target`

1. [ ] Are `Target`s a concept that's expressed in wit?
1. [ ] Can you target multiple things?
1. [ ] What is targetable? `Socket`? `Clocks`?

#### Import

1. [ ] Maybe you shouldn't be able to `open: true`.
    1. It can be annoying not to know where a definition came from.
    1. It might be a good idea to just have an explicit set of splatted names.
    1. If we're going to make people explicitly import the standard library then they're going to want `open: true`

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

### Standard library

1. [ ] Does this cover every function that wasm components an wwasm provide to us?
1. [ ] I'm not sure that the input/output stuff should be part of the standard library.
    1. [ ] By default wavelet should be working with absolutely no wasi.

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
