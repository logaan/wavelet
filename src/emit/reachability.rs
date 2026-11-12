use std::collections::HashSet;
use wasm_encoder::reencode::{Error, Reencode};
use wasmparser::{ElementItems, ExternalKind, Operator, Parser, Payload, TypeRef};

struct Graph {
    imports: u32,
    edges: Vec<Vec<u32>>,
    roots: Vec<u32>,
}

impl Graph {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut graph = Self {
            imports: 0,
            edges: Vec::new(),
            roots: Vec::new(),
        };
        for payload in Parser::new(0).parse_all(bytes) {
            match payload.map_err(|e| e.to_string())? {
                Payload::ImportSection(section) => {
                    for import in section.into_imports() {
                        if matches!(import.map_err(|e| e.to_string())?.ty, TypeRef::Func(_)) {
                            graph.imports += 1;
                            graph.edges.push(Vec::new());
                        }
                    }
                }
                Payload::ExportSection(section) => {
                    for export in section {
                        let export = export.map_err(|e| e.to_string())?;
                        if export.kind == ExternalKind::Func {
                            graph.roots.push(export.index);
                        }
                    }
                }
                Payload::StartSection { func, .. } => graph.roots.push(func),
                Payload::ElementSection(section) => {
                    for element in section {
                        match element.map_err(|e| e.to_string())?.items {
                            ElementItems::Functions(functions) => {
                                for function in functions {
                                    graph.roots.push(function.map_err(|e| e.to_string())?);
                                }
                            }
                            ElementItems::Expressions(_, expressions) => {
                                for expression in expressions {
                                    let expression = expression.map_err(|e| e.to_string())?;
                                    graph
                                        .roots
                                        .extend(references(expression.get_operators_reader())?);
                                }
                            }
                        }
                    }
                }
                Payload::GlobalSection(section) => {
                    for global in section {
                        let global = global.map_err(|e| e.to_string())?;
                        graph
                            .roots
                            .extend(references(global.init_expr.get_operators_reader())?);
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    graph.edges.push(references(
                        body.get_operators_reader().map_err(|e| e.to_string())?,
                    )?);
                }
                _ => {}
            }
        }
        Ok(graph)
    }

    fn live(&self) -> HashSet<u32> {
        let mut pending = self.roots.clone();
        let mut live = HashSet::new();
        while let Some(function) = pending.pop() {
            if live.insert(function) {
                pending.extend(self.edges[function as usize].iter().copied());
            }
        }
        live
    }
}

fn references(mut operators: wasmparser::OperatorsReader<'_>) -> Result<Vec<u32>, String> {
    let mut functions = Vec::new();
    while !operators.eof() {
        match operators.read().map_err(|e| e.to_string())? {
            Operator::Call { function_index }
            | Operator::ReturnCall { function_index }
            | Operator::RefFunc { function_index } => functions.push(function_index),
            _ => {}
        }
    }
    Ok(functions)
}

pub(super) fn live_functions(bytes: &[u8]) -> Result<HashSet<u32>, String> {
    Ok(Graph::parse(bytes)?.live())
}

pub(super) fn remove_unused(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let graph = Graph::parse(bytes)?;
    let live = graph.live();
    let mut next = 0;
    let indices = (0..graph.edges.len() as u32)
        .map(|old| {
            if old < graph.imports || live.contains(&old) {
                let index = next;
                next += 1;
                Some(index)
            } else {
                None
            }
        })
        .collect();
    let mut encoder = Reachable {
        indices,
        imports: graph.imports,
    };
    let mut module = wasm_encoder::Module::new();
    encoder
        .parse_core_module(&mut module, Parser::new(0), bytes)
        .map_err(|e| e.to_string())?;
    Ok(module.finish())
}

struct Reachable {
    indices: Vec<Option<u32>>,
    imports: u32,
}

impl Reencode for Reachable {
    type Error = String;

    fn function_index(&mut self, function: u32) -> Result<u32, Error<Self::Error>> {
        self.indices[function as usize]
            .ok_or_else(|| Error::UserError(format!("reference to removed function {function}")))
    }

    fn parse_function_section(
        &mut self,
        functions: &mut wasm_encoder::FunctionSection,
        section: wasmparser::FunctionSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        for (index, ty) in section.into_iter().enumerate() {
            if self.indices[self.imports as usize + index].is_some() {
                functions.function(ty?);
            }
        }
        Ok(())
    }

    fn parse_code_section(
        &mut self,
        code: &mut wasm_encoder::CodeSection,
        section: wasmparser::CodeSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        for (index, body) in section.into_iter().enumerate() {
            if self.indices[self.imports as usize + index].is_some() {
                self.parse_function_body(code, body?)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaps_calls_tail_calls_and_table_roots() {
        let bytes = wat::parse_str(
            r#"(module
            (type (func (result i32)))
            (import "host" "value" (func (type 0)))
            (func (result i32) i32.const 99)
            (func (result i32) call 0)
            (func (export "run") (result i32) return_call 2)
            (table 1 funcref)
            (elem (i32.const 0) func 2)
        )"#,
        )
        .unwrap();
        let output = remove_unused(&bytes).unwrap();
        wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all())
            .validate_all(&output)
            .unwrap();
        let graph = Graph::parse(&output).unwrap();
        assert_eq!(graph.imports, 1);
        assert_eq!(graph.edges, vec![vec![], vec![0], vec![1]]);
        assert!(graph.roots.contains(&1));
        assert!(graph.roots.contains(&2));
    }
}
