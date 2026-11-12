use super::*;

mod environment;
mod function_exports;
mod functions;
mod imports;
mod resource_exports;

use environment::initialize_emitter;
use function_exports::emit_function_exports;
use functions::{emit_function_bodies, reserve_functions};
use imports::{ResourceImports, register_imports};
use resource_exports::emit_resource_exports;

pub(crate) fn emit_core_module(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    let (mut em, user_res_forms) = initialize_emitter(arena, roots, info, deps)?;

    let imports = register_imports(&mut em, &user_res_forms)?;
    let n_imports = em.imports.len() as u32;
    let mut next = n_imports;
    let mut take = || {
        let i = next;
        next += 1;
        i
    };
    let order = reserve_functions(&mut em, roots, &user_res_forms, &imports, &mut take)?;

    emit_function_bodies(&mut em, &user_res_forms, &imports, &order)?;

    let mut exports = emit_function_exports(&mut em, &imports, &mut take)?;

    emit_resource_exports(&mut em, &user_res_forms, &imports, &mut exports, &mut take)?;
    Ok(serialize_module(&mut em, &exports))
}
