use super::CoreOptimizer;

pub(super) fn encode(
    module: Vec<u8>,
    wit: &str,
    world: &str,
    optimizer: &mut dyn CoreOptimizer,
) -> Result<Vec<u8>, String> {
    let mut resolve = wit_parser::Resolve::default();
    let package = resolve
        .push_str("wavelet-synthesized.wit", wit)
        .map_err(|e| format!("internal: synthesized WIT did not parse: {e:#}\n{wit}"))?;
    let world = resolve
        .select_world(&[package], Some(world))
        .map_err(|e| format!("internal: world selection failed: {e:#}"))?;
    let mut module = if scalar_world(&resolve, world) {
        optimizer.optimize_scalar_module(module)?
    } else {
        optimizer.optimize(module)?
    };
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

fn scalar_world(resolve: &wit_parser::Resolve, world: wit_parser::WorldId) -> bool {
    let world = &resolve.worlds[world];
    world.imports.is_empty()
        && world.exports.values().all(|item| match item {
            wit_parser::WorldItem::Function(function) => scalar_function(function),
            wit_parser::WorldItem::Interface { id, .. } => {
                let interface = &resolve.interfaces[*id];
                interface.types.is_empty() && interface.functions.values().all(scalar_function)
            }
            wit_parser::WorldItem::Type { .. } => false,
        })
}

fn scalar_function(function: &wit_parser::Function) -> bool {
    matches!(function.kind, wit_parser::FunctionKind::Freestanding)
        && function.params.iter().all(|param| scalar_type(param.ty))
        && function.result.is_none_or(scalar_type)
}

fn scalar_type(ty: wit_parser::Type) -> bool {
    use wit_parser::Type;
    matches!(
        ty,
        Type::Bool
            | Type::Char
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::S8
            | Type::S16
            | Type::S32
            | Type::S64
            | Type::F32
            | Type::F64
    )
}
