use super::Mode;
use crate::form::{Arena, Node, NodeId};
use crate::printer::print;

pub const PACKAGE: &str = "wavelet:snippet@0.1.0";
pub const INTERFACE: &str = "wavelet:snippet/api@0.1.0";
pub const FUNCTION: &str = "evaluate";

pub fn is_declaration(arena: &Arena, root: NodeId) -> bool {
    matches!(
        head(arena, root),
        Some(
            "package-MACRO"
                | "target-MACRO"
                | "import-MACRO"
                | "export-MACRO"
                | "def-MACRO"
                | "defmacro-MACRO"
                | "deftype-MACRO"
                | "defresource-MACRO"
                | "deffunctor-MACRO"
                | "instantiate-MACRO"
                | "derive-MACRO"
        )
    )
}

pub(super) fn wrap(
    arena: &Arena,
    roots: &[NodeId],
    source: &str,
    mode: Mode,
) -> Result<String, String> {
    let mut declarations = Vec::new();
    let mut expressions = Vec::new();
    let mut results = Vec::new();
    for &root in roots {
        if matches!(
            head(arena, root),
            Some("package-MACRO" | "target-MACRO" | "export-MACRO")
        ) {
            return Err("snippet mode accepts declarations and expressions; use project mode for Package, Target, or Export".into());
        }
        if is_declaration(arena, root) {
            declarations.push(print(arena, root));
            if let Some(name) = value_definition_name(arena, root) {
                expressions.push(name.to_owned());
            }
            if head(arena, root) == Some("defresource-MACRO")
                && let Node::Tup(items) = arena.node(root)
                && let Some(name) = items.get(1)
            {
                declarations.push(format!("Export {}", print(arena, *name)));
            }
        } else {
            expressions.push(print(arena, root));
        }
        if mode == Mode::Repl {
            let line = source.as_bytes()[..arena.span(root).0 as usize]
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count()
                + 1;
            let value = if let Some(name) = value_definition_name(arena, root) {
                format!("Do [{name} \"{{}}\"]")
            } else if is_declaration(arena, root) {
                "\"{}\"".into()
            } else {
                format!("to-string({})", print(arena, root))
            };
            results.push(format!("{{line: {line} value: {value}}}"));
        }
    }
    if mode == Mode::Repl {
        return Ok(format!(
            "Package \"{PACKAGE}\"\n{}\nDefType wavelet-repl-result {{line: u32 value: string}}\nExport {{name: {FUNCTION} result: list(wavelet-repl-result)}}\nDef {FUNCTION} Fn {{}} [{}]\n",
            declarations.join("\n"),
            results.join("\n")
        ));
    }
    let ends_in_expression = roots
        .last()
        .is_some_and(|root| !is_declaration(arena, *root));
    let body = if ends_in_expression {
        format!("to-string(Do [{}])", expressions.join("\n"))
    } else if expressions.is_empty() {
        "\"\"".into()
    } else {
        format!("Do [{}\n\"\"]", expressions.join("\n"))
    };
    Ok(format!(
        "Package \"{PACKAGE}\"\n{}\nExport {{name: {FUNCTION} result: string}}\nDef {FUNCTION} Fn {{}} {body}\n",
        declarations.join("\n")
    ))
}

pub fn value_definition_name(arena: &Arena, root: NodeId) -> Option<&str> {
    let Node::Tup(items) = arena.node(root) else {
        return None;
    };
    if items.len() != 3
        || head(arena, root) != Some("def-MACRO")
        || head(arena, items[2]) == Some("fn-MACRO")
    {
        return None;
    }
    match arena.node(items[1]) {
        Node::Sym(name) => Some(name),
        _ => None,
    }
}

fn head(arena: &Arena, root: NodeId) -> Option<&str> {
    let Node::Tup(items) = arena.node(root) else {
        return None;
    };
    match arena.node(*items.first()?) {
        Node::Sym(name) => Some(name),
        _ => None,
    }
}
