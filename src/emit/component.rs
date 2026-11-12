use super::macro_component::{emit_macro_core_module, macro_component_wit};
use super::{Arena, CoreOptimizer, NodeId};

pub(super) fn encode(
    module: Vec<u8>,
    wit: &str,
    world: &str,
    optimizer: &mut dyn CoreOptimizer,
) -> Result<Vec<u8>, String> {
    let mut module = optimizer.optimize(module)?;
    let mut resolve = wit_parser::Resolve::default();
    let package = resolve
        .push_str("wavelet-synthesized.wit", wit)
        .map_err(|e| format!("internal: synthesized WIT did not parse: {e:#}\n{wit}"))?;
    let world = resolve
        .select_world(&[package], Some(world))
        .map_err(|e| format!("internal: world selection failed: {e:#}"))?;
    wit_component::embed_component_metadata(
        &mut module,
        &resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )
    .map_err(|e| format!("embedding component metadata failed: {e:#}"))?;
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&module)
        .map_err(|e| format!("componentizing failed: {e:#}"))?
        .encode()
        .map_err(|e| format!("component encoding failed: {e:#}"))
}

pub fn emit_macro_component_with_optimizer(
    arena: &Arena,
    roots: &[NodeId],
    optimizer: &mut dyn CoreOptimizer,
) -> Result<Vec<u8>, String> {
    encode(
        emit_macro_core_module(arena, roots)?,
        &macro_component_wit(),
        "macro-lib",
        optimizer,
    )
}
