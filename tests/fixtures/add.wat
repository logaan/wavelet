;; Fixture for the Step 2 compile-time component runtime (src/host.rs).
;;
;; A trivial Component-Model component exporting one function,
;;   add: func(a: s32, b: s32) -> s32
;; so the host runtime's instantiate + call + result-marshalling path can be
;; smoke-tested without depending on the `tree` wire type or any other Wavelet
;; machinery.
;;
;; The committed `add.wasm` beside this file is generated from this source with
;; `wasm-tools` (https://github.com/bytecodealliance/wasm-tools):
;;
;;   wasm-tools parse tests/fixtures/add.wat -o tests/fixtures/add.wasm
;;
;; Regenerate it if you edit this file. The `.wasm` is checked in so the test
;; suite stays hermetic and needs no external tool at `cargo test` time.
(component
  (core module $m
    (func (export "add") (param i32 i32) (result i32)
      local.get 0
      local.get 1
      i32.add))
  (core instance $i (instantiate $m))
  (func $add (param "a" s32) (param "b" s32) (result s32)
    (canon lift (core func $i "add")))
  (export "add" (func $add)))
