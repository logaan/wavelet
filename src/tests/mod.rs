use super::*;

mod evaluation;
mod reading;
mod resources;
mod wit_synthesis;

#[cfg(not(target_arch = "wasm32"))]
mod emission;
#[cfg(not(target_arch = "wasm32"))]
mod foreign_macros;

fn eval_str(src: &str) -> String {
    let (arena, roots) = read_file(src).expect(src);
    let arena = std::rc::Rc::new(arena);
    let env = value::Env::root();
    builtins::install(&env);
    let env = env.child();
    let interp = interp::Interp::new();
    let mut last = value::unit();
    for root in roots {
        last = interp.eval(&arena, root, &env).expect(src);
    }
    value::print_value(&last)
}
