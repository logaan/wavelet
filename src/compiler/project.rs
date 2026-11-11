use std::collections::{HashMap, HashSet};

use super::{Artifact, ArtifactKind, unit::Unit};
use wac_graph::{CompositionGraph, EncodeOptions, types::Package};

pub(super) fn compose(
    entry: Option<&str>,
    units: &[Unit],
    index: &HashMap<String, usize>,
    artifacts: &mut Vec<Artifact>,
) -> Result<Option<String>, String> {
    let edges = units
        .iter()
        .map(|unit| {
            unit.info
                .imports
                .iter()
                .filter(|import| !crate::wit::is_macro_only(import))
                .filter_map(|import| index.get(&import.package).copied())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let imported = edges.iter().flatten().copied().collect::<HashSet<_>>();
    let root = match entry {
        Some(name) => units.iter().position(|unit| unit.name == name),
        None => units
            .iter()
            .enumerate()
            .find(|(i, unit)| !unit.macro_library && !imported.contains(i) && !edges[*i].is_empty())
            .or_else(|| {
                units
                    .iter()
                    .enumerate()
                    .find(|(i, unit)| !unit.macro_library && !imported.contains(i))
            })
            .map(|(i, _)| i),
    };
    let Some(root) = root else {
        if edges.iter().any(|edges| !edges.is_empty()) {
            return Err("cyclic component composition".into());
        }
        return Ok(None);
    };
    if edges[root].is_empty() {
        return Ok(Some(artifacts[root].name.clone()));
    }
    let mut order = Vec::new();
    visit(root, &edges, &mut vec![0; units.len()], &mut order)?;
    let mut graph = CompositionGraph::new();
    let mut ids = HashMap::new();
    for &position in &order {
        let artifact = &artifacts[position];
        let package = Package::from_bytes(
            &units[position].info.package_path,
            None,
            &artifact.bytes[..],
            graph.types_mut(),
        )
        .map_err(|e| format!("{}: {e:#}", artifact.name))?;
        let id = graph
            .register_package(package)
            .map_err(|e| format!("{}: {e:#}", artifact.name))?;
        ids.insert(position, id);
    }
    let socket = ids[&root];
    let plugs = order
        .into_iter()
        .filter(|position| *position != root)
        .map(|position| ids[&position])
        .collect::<Vec<_>>();
    wac_graph::plug(&mut graph, plugs, socket).map_err(|e| format!("compose: {e:#}"))?;
    let bytes = graph
        .encode(EncodeOptions::default())
        .map_err(|e| format!("compose: {e:#}"))?;
    let name = "app.wasm".to_owned();
    artifacts.push(Artifact {
        name: name.clone(),
        package_id: units[root].info.package.clone(),
        kind: ArtifactKind::Application,
        bytes,
    });
    Ok(Some(name))
}

fn visit(
    index: usize,
    edges: &[Vec<usize>],
    states: &mut [u8],
    order: &mut Vec<usize>,
) -> Result<(), String> {
    match states[index] {
        1 => return Err("cyclic component composition".into()),
        2 => return Ok(()),
        _ => {}
    }
    states[index] = 1;
    for &dependency in &edges[index] {
        visit(dependency, edges, states, order)?;
    }
    states[index] = 2;
    order.push(index);
    Ok(())
}
