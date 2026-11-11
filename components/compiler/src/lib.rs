mod protocol;
mod trees;

mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "compiler-component",
        generate_all,
    });
}

use bindings::exports::wavelet::compiler::compiler as api;
use wavelet::compiler as core;

struct Component;

struct MacroHost {
    libraries: Vec<api::MacroBinding>,
}

impl core::MacroHost for MacroHost {
    fn manifest(&mut self, key: &str) -> Option<Result<Vec<(String, u32)>, String>> {
        self.libraries
            .iter()
            .find(|binding| binding.key == key)
            .map(|binding| binding.library.manifest())
    }

    fn expand(
        &mut self,
        key: &str,
        name: &str,
        args: &wavelet::meta::Tree,
    ) -> Result<wavelet::meta::Tree, String> {
        let binding = self
            .libraries
            .iter()
            .find(|binding| binding.key == key)
            .ok_or_else(|| format!("macro capability `{key}` was not supplied"))?;
        binding
            .library
            .expand(name, &trees::to_guest(args))
            .map(trees::from_guest)
    }
}

impl api::Guest for Component {
    fn compile(
        request: api::Request,
        macros: Vec<api::MacroBinding>,
    ) -> Result<api::Step, Vec<api::Diagnostic>> {
        let mut host = MacroHost { libraries: macros };
        core::compile(&protocol::request(request), &mut host)
            .map(protocol::step)
            .map_err(|errors| errors.into_iter().map(protocol::diagnostic).collect())
    }
}

bindings::export!(Component with_types_in bindings);
