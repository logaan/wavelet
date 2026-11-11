use std::path::Path;

use wasmtime::component::{Component, Func, Instance, Linker};
use wasmtime::{Engine, Store};

pub use wasmtime::component::ComponentExportIndex;

pub use wasmtime::component::Val;

pub struct HostComponent {
    engine: Engine,
    store: Store<()>,
    instance: Instance,
}

impl HostComponent {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let engine = Engine::default();
        let component = Component::from_binary(&engine, bytes)
            .map_err(|e| format!("not a valid WebAssembly component: {e:#}"))?;
        Self::instantiate(engine, component)
    }

    pub fn from_file(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    fn instantiate(engine: Engine, component: Component) -> Result<Self, String> {
        let linker: Linker<()> = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        let instance = linker
            .instantiate(&mut store, &component)
            .map_err(|e| format!("failed to instantiate component: {e:#}"))?;
        Ok(HostComponent {
            engine,
            store,
            instance,
        })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn func(&mut self, name: &str) -> Result<Func, String> {
        self.instance
            .get_func(&mut self.store, name)
            .ok_or_else(|| format!("component has no exported function `{name}`"))
    }

    pub fn call(&mut self, name: &str, args: &[Val]) -> Result<Vec<Val>, String> {
        let func = self.func(name)?;
        self.invoke(func, name, args)
    }

    pub fn instance_func(&mut self, instance: &str, func: &str) -> Result<Func, String> {
        let inst_idx = self
            .instance
            .get_export_index(&mut self.store, None, instance)
            .ok_or_else(|| format!("component does not export the interface `{instance}`"))?;
        let func_idx = self
            .instance
            .get_export_index(&mut self.store, Some(&inst_idx), func)
            .ok_or_else(|| format!("interface `{instance}` has no function `{func}`"))?;
        self.instance
            .get_func(&mut self.store, func_idx)
            .ok_or_else(|| format!("export `{func}` of `{instance}` is not a function"))
    }

    pub fn call_instance(
        &mut self,
        instance: &str,
        func: &str,
        args: &[Val],
    ) -> Result<Vec<Val>, String> {
        let f = self.instance_func(instance, func)?;
        self.invoke(f, &format!("{instance}#{func}"), args)
    }

    pub fn drop_resource(&mut self, handle: Val) -> Result<(), String> {
        match handle {
            Val::Resource(r) => r
                .resource_drop(&mut self.store)
                .map_err(|e| format!("dropping resource failed: {e:#}")),
            other => Err(format!("not a resource handle: {other:?}")),
        }
    }

    fn invoke(&mut self, func: Func, name: &str, args: &[Val]) -> Result<Vec<Val>, String> {
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

    fn add_fixture() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/add.wasm"
        ))
        .expect("fixture add.wasm present")
    }

    #[test]
    fn instantiates_and_calls_export() {
        let mut comp = HostComponent::from_bytes(&add_fixture()).expect("fixture instantiates");
        let out = comp
            .call("add", &[Val::S32(2), Val::S32(40)])
            .expect("add call succeeds");
        assert_eq!(out, vec![Val::S32(42)]);

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

    fn load_err(bytes: &[u8]) -> String {
        match HostComponent::from_bytes(bytes) {
            Ok(_) => panic!("expected load to fail, but it succeeded"),
            Err(e) => e,
        }
    }

    #[test]
    fn non_component_bytes_are_actionable() {
        let err = load_err(b"\0asm not really");
        assert!(
            err.contains("not a valid WebAssembly component"),
            "unexpected error: {err}"
        );

        let core_module: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let err = load_err(core_module);
        assert!(
            err.contains("not a valid WebAssembly component"),
            "core module should not load as a component: {err}"
        );
    }
}
