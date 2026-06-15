# Nana / Wavelet ideas

## Web assembly components are a good target

### Wasm components make Wasm higher level

1. Wasm can run in a bunch of places.
1. A bunch of languages can produce Wasm.
1. But wasm itself is too low level to have good library style re-use.
1. Wasm components give us higher level pieces.

### The Wasm component ecosystem

1. It's got tools for package management, deployment, composition, etc.
   1. <https://github.com/yoshuawuyts/awesome-wasm-components>
1. It kind of drops your language into a pretty mature position from the get go.
1. The actual ecosystem of libraries is still in its infancy.
   1. <https://wa2.dev/registry> ??

### Core types are defined

1. The set of types are defined already.
1. Just anything that can be expressed in wit.
1. And the representation of them is defined by wave.

### This lets you join a large ecosystem with a small language

1. All of this means that with a fairly small language, and with putting
   relatively little effort into tooling, you can potentially build good
   software and deploy it in useful places.

## Components as dependency injection

1. If each file compiles to a component then you can use the same composition
   system for piecing together your app as you use for bringing in libraries.
1. You can unit test side-effecy components by giving them testing versions of
   the world rather than actual system wasi.

## Sandboxed components protecting from supply chain attacks

1. Your `left-pad` library shouldn't be able to read your disk or access the internet.

## Languages don't require many features to be expressive

1. Macros
1. TCO recursion
1. Closures
1. A reasonable set of literals
