use std::collections::BTreeMap;

use super::DependencyFile;
use crate::emit::Dep;
use wit_parser::{Resolve, SourceMap, decoding::DecodedWasm};

#[derive(Default)]
pub(super) struct Dependencies {
    resolve: Resolve,
}

impl Dependencies {
    pub(super) fn parse(files: &[DependencyFile]) -> Result<Self, String> {
        let mut resolve = Resolve::default();
        let mut groups = BTreeMap::<String, SourceMap>::new();
        let mut files: Vec<_> = files.iter().collect();
        files.sort_by(|a, b| a.name.cmp(&b.name));
        for file in files {
            if file.name.ends_with(".wasm") || file.name.ends_with(".wat") {
                let bytes =
                    wat::parse_bytes(&file.contents).map_err(|e| format!("{}: {e}", file.name))?;
                let decoded = wit_parser::decoding::decode(&bytes)
                    .map_err(|e| format!("{}: {e:#}", file.name))?;
                let decoded = match decoded {
                    DecodedWasm::WitPackage(resolve, _) | DecodedWasm::Component(resolve, _) => {
                        resolve
                    }
                };
                resolve
                    .merge(decoded)
                    .map_err(|e| format!("{}: {e:#}", file.name))?;
            } else if file.name.ends_with(".wit") {
                let text = std::str::from_utf8(&file.contents)
                    .map_err(|e| format!("{}: {e}", file.name))?;
                let group = file
                    .name
                    .rsplit_once('/')
                    .map(|(parent, _)| parent)
                    .unwrap_or(&file.name);
                groups
                    .entry(group.to_owned())
                    .or_default()
                    .push_str(&file.name, text);
            } else {
                return Err(format!(
                    "unsupported dependency file `{}`; expected .wit, .wasm, or .wat",
                    file.name
                ));
            }
        }
        let mut groups = groups
            .into_values()
            .map(|sources| {
                sources
                    .parse()
                    .map_err(|(map, error)| error.highlight(&map))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !groups.is_empty() {
            let first = groups.remove(0);
            resolve
                .push_groups(first, groups)
                .map_err(|e| format!("{e:#}"))?;
        }
        Ok(Self { resolve })
    }

    pub(super) fn resolve(&self, package: &str) -> Result<Option<Dep>, String> {
        crate::witdep::resolve_package(&self.resolve, package)
    }
}
