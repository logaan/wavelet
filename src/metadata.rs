use std::convert::Infallible;
use wasm_encoder::reencode::{Error, Reencode, ReencodeComponent};

pub fn strip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut stripper = Stripper;
    if wasmparser::Parser::is_component(bytes) {
        let mut component = wasm_encoder::Component::new();
        stripper
            .parse_component(&mut component, wasmparser::Parser::new(0), bytes)
            .map_err(|e| format!("stripping component metadata: {e}"))?;
        Ok(component.finish())
    } else {
        let mut module = wasm_encoder::Module::new();
        stripper
            .parse_core_module(&mut module, wasmparser::Parser::new(0), bytes)
            .map_err(|e| format!("stripping module metadata: {e}"))?;
        Ok(module.finish())
    }
}

fn removable(name: &str) -> bool {
    matches!(
        name,
        "name" | "component-name" | "producers" | "external_debug_info" | "sourceMappingURL"
    ) || name.starts_with(".debug_")
}

struct Stripper;

impl Reencode for Stripper {
    type Error = Infallible;

    fn parse_custom_section(
        &mut self,
        module: &mut wasm_encoder::Module,
        section: wasmparser::CustomSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        if !removable(section.name()) {
            wasm_encoder::reencode::utils::parse_custom_section(self, module, section)?;
        }
        Ok(())
    }
}

impl ReencodeComponent for Stripper {
    fn parse_component_custom_section(
        &mut self,
        component: &mut wasm_encoder::Component,
        section: wasmparser::CustomSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        if !removable(section.name()) {
            wasm_encoder::reencode::component_utils::parse_component_custom_section(
                self, component, section,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    fn section(name: &str) -> wasm_encoder::CustomSection<'_> {
        wasm_encoder::CustomSection {
            name: Cow::Borrowed(name),
            data: Cow::Borrowed(b"payload"),
        }
    }

    #[test]
    fn strips_nested_metadata_and_preserves_unknown_sections() {
        let mut module = wasm_encoder::Module::new();
        module.section(&section("producers"));
        module.section(&section("application-data"));
        module.section(&section("component-type:required"));
        let mut component = wasm_encoder::Component::new();
        component.section(&wasm_encoder::ModuleSection(&module));
        component.section(&section("component-name"));
        component.section(&section(".debug_info"));
        component.section(&section("application-config"));
        let output = strip(&component.finish()).unwrap();
        let names = wasmparser::Parser::new(0)
            .parse_all(&output)
            .filter_map(|payload| match payload.unwrap() {
                wasmparser::Payload::CustomSection(section) => Some(section.name().to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "application-data",
                "component-type:required",
                "application-config"
            ]
        );
        assert_eq!(strip(&output).unwrap(), output);
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(strip(b"invalid").is_err());
    }
}
