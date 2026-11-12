use std::collections::BTreeMap;
use std::convert::Infallible;
use wasm_encoder::reencode::{Error, Reencode};
use wasmparser::{ExternalKind, Parser, Payload};

pub(super) fn omit_support(original: Vec<u8>) -> Result<Vec<u8>, String> {
    if has_external_state(&original)? {
        return Ok(original);
    }
    let mut candidate = wasm_encoder::Module::new();
    SupportExports
        .parse_core_module(&mut candidate, Parser::new(0), &original)
        .map_err(|e| format!("scalar support exports: {e}"))?;
    let candidate = super::optimize(candidate.finish())?;
    if candidate.len() < original.len()
        && !has_runtime_state(&candidate)?
        && signatures(&candidate)? == signatures(&original)?
    {
        Ok(candidate)
    } else {
        Ok(original)
    }
}

fn has_external_state(bytes: &[u8]) -> Result<bool, String> {
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|e| e.to_string())? {
            Payload::ImportSection(section) if section.count() != 0 => return Ok(true),
            Payload::TableSection(section) if section.count() != 0 => return Ok(true),
            Payload::StartSection { .. } => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

fn has_runtime_state(bytes: &[u8]) -> Result<bool, String> {
    if has_external_state(bytes)? {
        return Ok(true);
    }
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|e| e.to_string())? {
            Payload::MemorySection(section) if section.count() != 0 => return Ok(true),
            Payload::GlobalSection(section) if section.count() != 0 => return Ok(true),
            Payload::DataSection(section) if section.count() != 0 => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

fn support_export(name: &str, kind: ExternalKind) -> bool {
    (kind == ExternalKind::Memory && name == "memory")
        || (kind == ExternalKind::Func
            && (name == "cabi_realloc" || name.starts_with("cabi_post_")))
}

fn signatures(bytes: &[u8]) -> Result<BTreeMap<String, wasmparser::FuncType>, String> {
    let mut types = Vec::new();
    let mut functions = Vec::new();
    let mut exports = BTreeMap::new();
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|e| e.to_string())? {
            Payload::TypeSection(section) => {
                for ty in section.into_iter_err_on_gc_types() {
                    types.push(ty.map_err(|e| e.to_string())?);
                }
            }
            Payload::FunctionSection(section) => {
                for ty in section {
                    functions.push(ty.map_err(|e| e.to_string())?);
                }
            }
            Payload::ExportSection(section) => {
                for export in section {
                    let export = export.map_err(|e| e.to_string())?;
                    if export.kind == ExternalKind::Func
                        && !support_export(export.name, export.kind)
                    {
                        exports.insert(
                            export.name.to_owned(),
                            types[functions[export.index as usize] as usize].clone(),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(exports)
}

struct SupportExports;

impl Reencode for SupportExports {
    type Error = Infallible;

    fn parse_export_section(
        &mut self,
        exports: &mut wasm_encoder::ExportSection,
        section: wasmparser::ExportSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        for export in section {
            let export = export?;
            if !support_export(export.name, export.kind) {
                self.parse_export(exports, export)?;
            }
        }
        Ok(())
    }
}
