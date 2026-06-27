//! A compile-time runtime for Component-Model `.wasm` modules.
//!
//! Running a macro that lives in *another* component (design.md §6.3) means the
//! compiler must **execute** that component at build time: instantiate it, call
//! an exported function with runtime-typed arguments, and read back a
//! runtime-typed result. The project's other native crates (`wac-graph`,
//! `wasm-encoder`, `wit-*`) compose and describe components but never run them;
//! this module is the missing executor. Step 3 layers the
//! `wavelet:meta/macros` contract (`manifest`/`expand`) on top of the generic
//! call surface here.
//!
//! ## Design choices
//!
//! - **Runtime: `wasmtime`** with the `component-model` feature, driven through
//!   its *dynamic* typed-value API (`component::Val` / `Func::call(&[Val])`). We
//!   deliberately do **not** generate bindings to a fixed world, because the
//!   macro contract is small and uniform and a host that loads arbitrary macro
//!   libraries benefits from calling exports by name with dynamic values.
//!
//! - **Sandboxed by construction.** Each component is instantiated against an
//!   **empty `Linker`** — no WASI, no filesystem, no clock, no randomness, no
//!   host functions at all. A macro guest therefore gets zero ambient
//!   capabilities, which keeps builds deterministic and untrusted macros
//!   contained (design.md §6.3). Granting a capability later is a deliberate,
//!   explicit act, not something wired in here.
//!
//! - **Native-only.** Like `emit`/`build`/`wit`/`tools`, this module is gated
//!   `#[cfg(not(target_arch = "wasm32"))]` in `lib.rs`, and `wasmtime` is pulled
//!   in only under the `cfg(not(target_arch = "wasm32"))` dependency table — it
//!   must never reach the docs-playground `cdylib` build.
//!
//! - **Error convention.** Every fallible entry point returns
//!   `Result<_, String>` with an actionable message, matching
//!   `build.rs`/`emit.rs`/`tools.rs` so callers `?`/`map_err` uniformly.
//!
//! Re-exporting [`Val`] lets callers build and match argument/result values
//! without depending on `wasmtime` directly.

use std::path::Path;

use wasmtime::component::{Component, Func, Instance, Linker};
use wasmtime::{Engine, Store};

/// Index of an export located by name, reused to look up nested exports (a
/// function *inside* an exported interface instance). Re-exported so callers can
/// hold one across `get_export`/`get_func` calls without naming `wasmtime`.
pub use wasmtime::component::ComponentExportIndex;

/// The dynamic component value type, re-exported so callers marshal arguments
/// and results without naming `wasmtime` themselves.
pub use wasmtime::component::Val;

/// A loaded, instantiated Component-Model component ready to have its exports
/// called.
///
/// Owns the `wasmtime` `Engine`, `Store`, and `Instance`. The store carries no
/// host state (`()`), reflecting the capability-free sandbox: the guest has
/// nothing to call back into.
pub struct HostComponent {
    engine: Engine,
    store: Store<()>,
    instance: Instance,
}

impl HostComponent {
    /// Instantiate a component from its raw `.wasm` bytes.
    ///
    /// The bytes must be an encoded **component** (not a bare core module); a
    /// core module — or anything that isn't a valid component — yields an
    /// actionable error rather than a panic. Instantiation uses an empty,
    /// capability-free linker (no WASI, no host imports), so a component that
    /// imports anything will fail here with the missing-import reported by
    /// `wasmtime`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let engine = Engine::default();
        let component = Component::from_binary(&engine, bytes).map_err(|e| {
            format!("not a valid WebAssembly component: {e:#}")
        })?;
        Self::instantiate(engine, component)
    }

    /// Instantiate a component read from a `.wasm` file on disk.
    ///
    /// Reads `path`, then defers to [`HostComponent::from_bytes`]. A read
    /// failure is reported with the path; a decode/instantiate failure carries
    /// `wasmtime`'s diagnostic.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    /// Shared tail of the constructors: build the empty linker, instantiate, and
    /// wrap up the engine/store/instance.
    fn instantiate(engine: Engine, component: Component) -> Result<Self, String> {
        // An empty linker: the guest gets no host imports of any kind. This is
        // the sandbox. If a component imports something, instantiation fails
        // here with a clear missing-import error.
        let linker: Linker<()> = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        let instance = linker
            .instantiate(&mut store, &component)
            .map_err(|e| format!("failed to instantiate component: {e:#}"))?;
        Ok(HostComponent { engine, store, instance })
    }

    /// The underlying `wasmtime` engine, for callers that need to construct
    /// component values tied to it (rarely needed; most use [`Val`] directly).
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Look up an exported function by name.
    ///
    /// Returns an actionable error naming the missing export when there is no
    /// such function (or the export exists but isn't a function). Use
    /// [`HostComponent::call`] for the common look-up-then-invoke path.
    pub fn func(&mut self, name: &str) -> Result<Func, String> {
        self.instance
            .get_func(&mut self.store, name)
            .ok_or_else(|| format!("component has no exported function `{name}`"))
    }

    /// Call an exported function by name with dynamically-typed arguments,
    /// returning its results.
    ///
    /// Marshals `args` straight to the guest, invokes it, and hands back the
    /// result values. Arity/type mismatches between `args` and the export's
    /// signature surface as an actionable error from `wasmtime` rather than a
    /// panic. Most Component-Model functions return exactly one value, but the
    /// general shape is a vector so multi-result and zero-result signatures both
    /// work. (`post_return` cleanup is handled internally by `wasmtime` 45, so
    /// the same instance can be called repeatedly.)
    pub fn call(&mut self, name: &str, args: &[Val]) -> Result<Vec<Val>, String> {
        let func = self.func(name)?;
        self.invoke(func, name, args)
    }

    /// Look up a function exported *inside an exported interface instance*.
    ///
    /// Component-Model interface exports are nested: a component exporting an
    /// interface like `wavelet:meta/macros@0.1.0` surfaces it as an instance
    /// export, and the interface's functions (`manifest`, `expand`) live one
    /// level down inside that instance — they are **not** top-level funcs, so
    /// [`HostComponent::func`] can't reach them. This resolves the instance by
    /// name, then the function within it. An actionable error names whichever of
    /// the two is missing.
    pub fn instance_func(&mut self, instance: &str, func: &str) -> Result<Func, String> {
        let inst_idx = self
            .instance
            .get_export_index(&mut self.store, None, instance)
            .ok_or_else(|| {
                format!("component does not export the interface `{instance}`")
            })?;
        let func_idx = self
            .instance
            .get_export_index(&mut self.store, Some(&inst_idx), func)
            .ok_or_else(|| {
                format!("interface `{instance}` has no function `{func}`")
            })?;
        self.instance
            .get_func(&mut self.store, &func_idx)
            .ok_or_else(|| {
                format!("export `{func}` of `{instance}` is not a function")
            })
    }

    /// Call a function exported inside an exported interface instance (the
    /// nested-export analogue of [`HostComponent::call`]).
    pub fn call_instance(
        &mut self,
        instance: &str,
        func: &str,
        args: &[Val],
    ) -> Result<Vec<Val>, String> {
        let f = self.instance_func(instance, func)?;
        self.invoke(f, &format!("{instance}#{func}"), args)
    }

    /// Explicitly drop an exported resource handle, running the guest's
    /// destructor (`[dtor]`) for it. The dynamic `Val::Resource` API never frees
    /// a handle on its own, so a host that wants to observe a guest dtor (e.g.
    /// the functor ABI spike / parity tests) must call this. Surfaces wasmtime's
    /// diagnostic on failure (e.g. a trapping dtor).
    pub fn drop_resource(&mut self, handle: Val) -> Result<(), String> {
        match handle {
            Val::Resource(r) => r
                .resource_drop(&mut self.store)
                .map_err(|e| format!("dropping resource failed: {e:#}")),
            other => Err(format!("not a resource handle: {other:?}")),
        }
    }

    /// Shared invoke tail: size the result buffer to the export's declared
    /// result count, call, and surface failures with `name` for context.
    fn invoke(&mut self, func: Func, name: &str, args: &[Val]) -> Result<Vec<Val>, String> {
        // Size the result buffer to the export's declared result count so
        // `Func::call` can write directly into it.
        let result_count = func.ty(&self.store).results().len();
        let mut results = vec![Val::Bool(false); result_count];
        func.call(&mut self.store, args, &mut results)
            .map_err(|e| format!("call to `{name}` failed: {e:#}"))?;
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checked-in fixture: a trivial component exporting
    /// `add: func(a: s32, b: s32) -> s32`. Built from `tests/fixtures/add.wat`
    /// with `wasm-tools` (see that file's header); kept tiny so the unit suite
    /// stays hermetic and needs no external tool to run.
    fn add_fixture() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/add.wasm"
        ))
        .expect("fixture add.wasm present")
    }

    #[test]
    fn instantiates_and_calls_export() {
        let mut comp = HostComponent::from_bytes(&add_fixture())
            .expect("fixture instantiates");
        let out = comp
            .call("add", &[Val::S32(2), Val::S32(40)])
            .expect("add call succeeds");
        assert_eq!(out, vec![Val::S32(42)]);
        // A second call on the same instance works (post_return cleanup ran).
        let out = comp.call("add", &[Val::S32(-5), Val::S32(5)]).unwrap();
        assert_eq!(out, vec![Val::S32(0)]);
    }

    #[test]
    fn missing_export_is_actionable() {
        let mut comp = HostComponent::from_bytes(&add_fixture()).unwrap();
        let err = comp.call("subtract", &[]).unwrap_err();
        assert!(
            err.contains("no exported function `subtract`"),
            "unexpected error: {err}"
        );
    }

    /// `HostComponent` wraps non-`Debug` `wasmtime` handles, so `unwrap_err`
    /// (which needs `T: Debug`) won't compile. Pull the error out by hand.
    fn load_err(bytes: &[u8]) -> String {
        match HostComponent::from_bytes(bytes) {
            Ok(_) => panic!("expected load to fail, but it succeeded"),
            Err(e) => e,
        }
    }

    #[test]
    fn non_component_bytes_are_actionable() {
        // Not a component at all — random bytes.
        let err = load_err(b"\0asm not really");
        assert!(
            err.contains("not a valid WebAssembly component"),
            "unexpected error: {err}"
        );

        // A *core module* (valid wasm, but not a component) must also be
        // rejected by the component loader, not silently accepted. The empty
        // core module is just the wasm preamble: magic `\0asm` + version 1.
        let core_module: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let err = load_err(core_module);
        assert!(
            err.contains("not a valid WebAssembly component"),
            "core module should not load as a component: {err}"
        );
    }
}
