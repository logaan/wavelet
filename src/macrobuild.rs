use crate::form::{Arena, Node, NodeId};

pub fn is_macro_library(arena: &Arena, roots: &[NodeId]) -> bool {
    let mut saw_macro = false;
    let mut saw_package = false;
    for &root in roots {
        let Node::Tup(items) = arena.node(root) else {
            return false;
        };
        let Some(&head) = items.first() else {
            return false;
        };
        let Node::Sym(head_name) = arena.node(head) else {
            return false;
        };
        match head_name.as_str() {
            "package-MACRO" => saw_package = true,
            "defmacro-MACRO" => saw_macro = true,

            _ => return false,
        }
    }
    saw_package && saw_macro
}

pub fn build_macro_component(arena: &Arena, roots: &[NodeId]) -> Result<Vec<u8>, String> {
    crate::emit::emit_macro_component(arena, roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::read_file;

    fn roots(src: &str) -> (Arena, Vec<NodeId>) {
        read_file(src).expect("read")
    }

    #[test]
    fn pure_macro_file_is_a_macro_library() {
        let (a, r) = roots(
            "Package \"demo:macros@0.1.0\"\n\
             DefMacro identity {x} x\n\
             DefMacro unless {cond body}\n  Quasi If Unquote(cond) {} Unquote(body)\n",
        );
        assert!(is_macro_library(&a, &r));
    }

    #[test]
    fn file_with_export_is_not_a_macro_library() {
        let (a, r) = roots(
            "Package \"demo:app@0.1.0\"\n\
             DefMacro identity {x} x\n\
             Export greet\n\
             Def greet Fn {} \"hi\"\n",
        );
        assert!(!is_macro_library(&a, &r));
    }

    #[test]
    fn file_with_no_macros_is_not_a_macro_library() {
        let (a, r) = roots("Package \"demo:app@0.1.0\"\n");
        assert!(!is_macro_library(&a, &r));
    }

    #[test]
    fn file_with_runtime_def_is_not_a_macro_library() {
        let (a, r) = roots(
            "Package \"demo:app@0.1.0\"\n\
             DefMacro identity {x} x\n\
             Def x 1\n",
        );
        assert!(!is_macro_library(&a, &r));
    }
}
