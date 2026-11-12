use std::path::{Path, PathBuf};

use lsp_types::{
    CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, DocumentSymbol, Hover,
    HoverContents, MarkupContent, MarkupKind, Range, SymbolKind,
};
use wavelet::form::{Arena, Node, NodeId};

use crate::line_index::LineIndex;

const SPECIAL_FORMS: &[(&str, &str)] = &[
    (
        "Package",
        "Package \"ns:name@ver\" — declare the component's package id",
    ),
    (
        "Import",
        "Import {pkg: \"…\" as: alias} — import another component's interface",
    ),
    (
        "Instantiate",
        "Instantiate {pkg: \"…\" with: {elem: t} as: alias} — apply a compile-time functor",
    ),
    (
        "DefFunctor",
        "DefFunctor \"ns:name\" {type-params} [declarations] — define a monomorphic template",
    ),
    (
        "Export",
        "Export name — export a function from this component",
    ),
    ("DefType", "DefType Name type — define a named WIT type"),
    ("Def", "Def name value — bind a top-level name"),
    ("Fn", "Fn {params} body — a closure"),
    (
        "If",
        "If cond then else — conditional (stdlib macro; expands to `Match The bool cond …`)",
    ),
    ("Let", "Let {bindings} body — local bindings"),
    (
        "Do",
        "Do [a b …] — evaluate expressions in sequence; value is the last (stdlib macro over `Match`)",
    ),
    ("Match", "Match value (pattern → result …) — pattern match"),
    ("Quote", "Quote form — the form as data"),
    (
        "Quasi",
        "Quasi form — quasiquote (template with Unquote/Splice)",
    ),
    ("Unquote", "Unquote form — escape a Quasi template"),
    (
        "Splice",
        "Splice form — splice a list into a Quasi template",
    ),
    ("DefMacro", "DefMacro Name {params} body — define a macro"),
    ("The", "The type value — a type ascription"),
];

fn builtin_doc(name: &str) -> &'static str {
    match name {
        "eq" => "Structural equality.",
        "lt" | "le" | "gt" | "ge" => "Ordered comparison.",
        "not" => "Boolean negation.",
        "add" | "sub" | "mul" => "Wasm arithmetic; integer results wrap modulo 2^64.",
        "div" => "Wasm signed integer or floating division.",
        "rem" => "Wasm signed integer remainder; floating operands are rejected.",
        "neg" => "Numeric negation; integers wrap modulo 2^64.",
        "min" | "max" => "Smaller / larger of two values.",
        "abs" => "Absolute value.",
        "len" => "Length of a list or string.",
        "empty" => "Whether a list or string is empty.",
        "get" | "put" => "Index / replace by index.",
        "push" => "Append an element to a list.",
        "concat" => "Concatenate two lists.",
        "head" | "tail" => "First element / everything but the first.",
        "reverse" => "Reverse a list.",
        "range" => "List of integers in a range.",
        "map" | "filter" | "fold" => "Higher-order list operation.",
        "zip" => "Pair up two lists.",
        "str-cat" => "Concatenate strings.",
        "split" | "join" => "Split a string / join a list of strings.",
        "contains" => "Substring / membership test.",
        "to-string" => "Render a value as a string.",
        "to-char" => "Convert an integer Unicode scalar value to a char.",
        "gensym" => "A fresh, unique symbol (for macros).",
        "expand" => "Macro-expand a form.",
        "some" | "ok" | "err" => "Construct an option / result variant.",
        "cell-new" | "cell-get" | "cell-set" => "Mutable cell operations.",
        "drop" => "Drop a resource handle.",
        _ if wavelet::builtins::tuple_ctor_arity(name).is_some() => {
            "Construct a tuple of this fixed arity."
        }
        _ => "Builtin function.",
    }
}

pub fn diagnostics(text: &str, index: &LineIndex) -> Vec<Diagnostic> {
    match wavelet::read_file(text) {
        Ok((arena, roots)) => check_diagnostics(text, index, arena, roots),
        Err(e) => {
            let at = (e.at as usize).min(text.len());
            let start = index.position(text, at);

            let end_off = next_char_boundary(text, at);
            let end = if end_off > at {
                index.position(text, end_off)
            } else if at > 0 {
                let s = index.position(text, prev_char_boundary(text, at));
                return vec![diag(Range::new(s, start), e.msg)];
            } else {
                start
            };
            vec![diag(Range::new(start, end), e.msg)]
        }
    }
}

fn check_diagnostics(
    text: &str,
    index: &LineIndex,
    arena: Arena,
    roots: Vec<NodeId>,
) -> Vec<Diagnostic> {
    let (arena, roots) = match wavelet::expand::expand_file(arena, &roots, None) {
        Ok(pair) => pair,
        Err(_) => match wavelet::read_file(text) {
            Ok(pair) => pair,
            Err(_) => return Vec::new(),
        },
    };
    match wavelet::check::check_program(&arena, &roots) {
        Ok(()) => Vec::new(),
        Err(msg) => {
            let message = msg.strip_prefix("eval error: ").unwrap_or(&msg).to_string();
            let start = index.position(text, 0);
            let end = index.position(text, next_char_boundary(text, 0));
            vec![diag(Range::new(start, end), message)]
        }
    }
}

fn diag(range: Range, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("wavelet".into()),
        message,
        ..Default::default()
    }
}

pub fn completions(text: &str, path: Option<&Path>) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    for (title, detail) in SPECIAL_FORMS {
        items.push(CompletionItem {
            label: title.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("special form".into()),
            documentation: Some(doc_string(detail)),
            ..Default::default()
        });
    }

    for name in wavelet::builtins::NAMES {
        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some("builtin".into()),
            documentation: Some(doc_string(builtin_doc(name))),
            ..Default::default()
        });
    }

    if let Ok((arena, roots)) = wavelet::read_file(text) {
        for (name, kind, _) in definitions(&arena, &roots) {
            items.push(CompletionItem {
                label: name,
                kind: Some(match kind {
                    SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
                    SymbolKind::STRUCT => CompletionItemKind::STRUCT,
                    _ => CompletionItemKind::VARIABLE,
                }),
                detail: Some("defined here".into()),
                ..Default::default()
            });
        }

        items.extend(imported_completions(&arena, &roots, path));
    }

    items
}

fn imported_completions(
    arena: &Arena,
    roots: &[NodeId],
    path: Option<&Path>,
) -> Vec<CompletionItem> {
    let Some(deps_dir) = path.and_then(wit_deps_dir) else {
        return Vec::new();
    };
    if !deps_dir.is_dir() {
        return Vec::new();
    }
    let Ok(info) = wavelet::wit::collect(arena, roots) else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for import in &info.imports {
        let Ok(Some(dep)) = wavelet::witdep::resolve_dep(&deps_dir, &import.package) else {
            continue;
        };
        for func in &dep.funcs {
            items.push(CompletionItem {
                label: format!("{}/{}", import.alias, func.name),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(format!("imported from {}", import.package)),
                documentation: Some(doc_string(&func.to_wit())),
                ..Default::default()
            });
        }
    }
    items
}

fn wit_deps_dir(file: &Path) -> Option<PathBuf> {
    let src_dir = file.parent()?;
    let root = src_dir.parent().unwrap_or(src_dir);
    Some(root.join("wit").join("deps"))
}

pub fn hover(text: &str, index: &LineIndex, offset: usize) -> Option<Hover> {
    let (arena, _roots) = wavelet::read_file(text).ok()?;
    let node_id = smallest_node_at(&arena, offset)?;
    let name = match arena.node(node_id) {
        Node::Sym(s) => s.clone(),
        _ => return None,
    };

    if name.ends_with("-MACRO")
        && let Some((title, detail)) = SPECIAL_FORMS
            .iter()
            .find(|(t, _)| wavelet::lexer::title_to_macro_name(t) == name)
    {
        return Some(markdown_hover(
            format!("**{title}** — special form\n\n{detail}"),
            arena.span(node_id),
            text,
            index,
        ));
    }

    if wavelet::builtins::NAMES.contains(&name.as_str()) {
        return Some(markdown_hover(
            format!("**{name}** — builtin\n\n{}", builtin_doc(&name)),
            arena.span(node_id),
            text,
            index,
        ));
    }

    None
}

pub fn document_symbols(text: &str, index: &LineIndex) -> Vec<DocumentSymbol> {
    let Ok((arena, roots)) = wavelet::read_file(text) else {
        return Vec::new();
    };
    definitions(&arena, &roots)
        .into_iter()
        .map(|(name, kind, call_id)| {
            let range = span_to_range(arena.span(call_id), text, index);
            let sel = match def_name_node(&arena, call_id) {
                Some(n) => span_to_range(arena.span(n), text, index),
                None => range,
            };
            #[allow(deprecated)]
            DocumentSymbol {
                name,
                detail: None,
                kind,
                tags: None,
                deprecated: None,
                range,
                selection_range: sel,
                children: None,
            }
        })
        .collect()
}

fn definitions(arena: &Arena, roots: &[NodeId]) -> Vec<(String, SymbolKind, NodeId)> {
    let mut out = Vec::new();
    for &root in roots {
        let Node::Tup(items) = arena.node(root) else {
            continue;
        };
        let Some(&head) = items.first() else { continue };
        let kind = match sym_name(arena, head).as_deref() {
            Some("def-MACRO") => SymbolKind::FUNCTION,
            Some("deftype-MACRO") => SymbolKind::STRUCT,
            Some("defmacro-MACRO") => SymbolKind::OPERATOR,
            _ => continue,
        };
        if let Some(name) = def_name_node(arena, root).and_then(|n| sym_name(arena, n)) {
            out.push((name, kind, root));
        }
    }
    out
}

fn def_name_node(arena: &Arena, call_id: NodeId) -> Option<NodeId> {
    let Node::Tup(items) = arena.node(call_id) else {
        return None;
    };
    items.get(1).copied()
}

fn sym_name(arena: &Arena, id: NodeId) -> Option<String> {
    match arena.node(id) {
        Node::Sym(s) => Some(s.clone()),
        _ => None,
    }
}

fn smallest_node_at(arena: &Arena, offset: usize) -> Option<NodeId> {
    let mut best: Option<(NodeId, u32)> = None;
    for id in 0..arena.nodes.len() as NodeId {
        let (s, e) = arena.span(id);
        if (s as usize) <= offset && offset < (e as usize) {
            let width = e - s;
            if best.is_none_or(|(_, w)| width < w) {
                best = Some((id, width));
            }
        }
    }
    best.map(|(id, _)| id)
}

fn span_to_range(span: (u32, u32), text: &str, index: &LineIndex) -> Range {
    Range::new(
        index.position(text, span.0 as usize),
        index.position(text, span.1 as usize),
    )
}

fn markdown_hover(value: String, span: (u32, u32), text: &str, index: &LineIndex) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(span_to_range(span, text, index)),
    }
}

fn doc_string(s: &str) -> lsp_types::Documentation {
    lsp_types::Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: s.to_string(),
    })
}

fn next_char_boundary(text: &str, mut i: usize) -> usize {
    if i >= text.len() {
        return text.len();
    }
    i += 1;
    while i < text.len() && !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn prev_char_boundary(text: &str, mut i: usize) -> usize {
    i = i.saturating_sub(1);
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(text: &str, path: Option<&Path>) -> Vec<String> {
        completions(text, path)
            .into_iter()
            .map(|c| c.label)
            .collect()
    }

    #[test]
    fn removed_builtins_and_target_are_gone() {
        let got = labels("", None);
        for dead in [
            "Target",
            "print",
            "println",
            "read-line",
            "args",
            "env",
            "upper",
            "lower",
        ] {
            assert!(
                !got.iter().any(|l| l == dead),
                "completion still offers removed `{dead}`: {got:?}"
            );
        }

        assert!(got.iter().any(|l| l == "Import"));
        assert!(got.iter().any(|l| l == "map"));
    }

    #[test]
    fn type_errors_surface_as_diagnostics() {
        let index = LineIndex::new("");
        let bad = "Def f Fn {} add(\"a\" \"b\")\n";
        let got = diagnostics(bad, &LineIndex::new(bad));
        assert_eq!(got.len(), 1, "expected one checker diagnostic: {got:?}");
        assert!(
            got[0].message.contains("numeric"),
            "message should be the compiler's: {}",
            got[0].message
        );

        let _ = index;
        let ok = "Def f Fn {n: s64} add(n 1)\n";
        let got = diagnostics(ok, &LineIndex::new(ok));
        assert!(got.is_empty(), "well-typed file should be clean: {got:?}");
    }

    #[test]
    fn non_exhaustive_match_is_a_diagnostic() {
        let src = "Def f Fn {b: bool} Match b [(true 1)]\n";
        let got = diagnostics(src, &LineIndex::new(src));
        assert_eq!(got.len(), 1);
        assert!(
            got[0].message.contains("missing case `false`"),
            "got: {}",
            got[0].message
        );
    }

    #[test]
    fn import_resolves_functions_from_wit_deps() {
        let dir = std::env::temp_dir().join(format!("wavelet-lsp-analysis-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let deps = dir.join("wit").join("deps");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&deps).unwrap();
        std::fs::write(
            deps.join("acme-greet.wit"),
            "package acme:greet@0.1.0;\n\
             interface api {\n  \
               greet: func(name: string) -> string;\n\
             }\n",
        )
        .unwrap();

        let file = src.join("main.wlt");
        let text = "Package \"demo:app@0.1.0\"\n\
                    Import {pkg: \"acme:greet/api\" as: greeting}\n";

        let got = labels(text, Some(&file));
        assert!(
            got.iter().any(|l| l == "greeting/greet"),
            "expected `greeting/greet` from wit/deps import: {got:?}"
        );

        let no_path = labels(text, None);
        assert!(!no_path.iter().any(|l| l == "greeting/greet"));
        assert!(no_path.iter().any(|l| l == "Import"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
