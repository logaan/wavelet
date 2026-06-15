//! Language analysis built on top of the `wavelet` compiler crate.
//!
//! Every feature here funnels through `wavelet::read_file`, so the server and
//! the compiler can never disagree about what parses: the reader is the single
//! source of syntax truth (CLAUDE.md, "the interpreter is the semantics
//! oracle"). We deliberately stop at *read* — no expansion, evaluation, or
//! codegen — to keep responses fast and side-effect free.

use std::path::{Path, PathBuf};

use lsp_types::{
    CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, DocumentSymbol,
    Hover, HoverContents, MarkupContent, MarkupKind, Range, SymbolKind,
};
use wavelet::form::{Arena, Node, NodeId};

use crate::line_index::LineIndex;

/// User-facing TitleCase special forms (the core macro table, §2.4) with a
/// one-line summary. The reader stores these heads as `name-MACRO` symbols.
const SPECIAL_FORMS: &[(&str, &str)] = &[
    ("Package", "Package \"ns:name@ver\" — declare the component's package id"),
    ("Import", "Import {pkg: \"…\" as: alias} — import another component's interface"),
    ("Export", "Export name — export a function from this component"),
    ("DefType", "DefType Name type — define a named WIT type"),
    ("Def", "Def name value — bind a top-level name"),
    ("Fn", "Fn {params} body — a closure"),
    ("If", "If cond then else — conditional"),
    ("Let", "Let {bindings} body — local bindings"),
    ("Do", "Do (a b …) — evaluate forms in sequence"),
    ("Match", "Match value (pattern → result …) — pattern match"),
    ("Quote", "Quote form — the form as data"),
    ("Quasi", "Quasi form — quasiquote (template with Unquote/Splice)"),
    ("Unquote", "Unquote form — escape a Quasi template"),
    ("Splice", "Splice form — splice a list into a Quasi template"),
    ("DefMacro", "DefMacro Name {params} body — define a macro"),
    ("The", "The type value — a type ascription"),
];

/// Short descriptions for the standard builtins (`builtins::NAMES`). Names not
/// listed here still complete, just without a custom blurb.
fn builtin_doc(name: &str) -> &'static str {
    match name {
        "eq" => "Structural equality.",
        "lt" | "le" | "gt" | "ge" => "Ordered comparison.",
        "not" => "Boolean negation.",
        "add" | "sub" | "mul" | "div" | "rem" => "Integer/decimal arithmetic.",
        "neg" => "Numeric negation.",
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
        "upper" | "lower" => "Change string case.",
        "split" | "join" => "Split a string / join a list of strings.",
        "contains" => "Substring / membership test.",
        "to-string" => "Render a value as a string.",
        "read" => "Parse a string into a form.",
        "apply" => "Call a function with a list of arguments.",
        "gensym" => "A fresh, unique symbol (for macros).",
        "expand" => "Macro-expand a form.",
        "some" | "ok" | "err" => "Construct an option / result variant.",
        "cell-new" | "cell-get" | "cell-set" => "Mutable cell operations.",
        "drop" => "Drop a resource handle.",
        _ => "Builtin function.",
    }
}

/// Syntax diagnostics: a single error from the reader, if reading fails.
pub fn diagnostics(text: &str, index: &LineIndex) -> Vec<Diagnostic> {
    match wavelet::read_file(text) {
        Ok(_) => Vec::new(),
        Err(e) => {
            let at = (e.at as usize).min(text.len());
            let start = index.position(text, at);
            // Highlight the offending character; fall back to the previous one
            // at end-of-input so the squiggle stays visible.
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

fn diag(range: Range, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("wavelet".into()),
        message,
        ..Default::default()
    }
}

/// Completions: special forms, builtins, names defined in this document, and —
/// when the document's path is known — the functions of every external WIT
/// package it `Import`s, resolved from the project's `wit/deps` directory.
/// Context-insensitive — a deliberately basic offering.
///
/// `path` is the document's filesystem path (from its `file://` URI), used only
/// to locate `wit/deps`; pass `None` for untitled/in-memory buffers, which
/// simply skips the import-resolution completions.
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

/// Completions contributed by the document's `Import`s: each imported external
/// WIT package (resolved from `wit/deps`) offers its functions, labelled the way
/// they are called in source — `alias/func-name` (e.g. `http/body`).
///
/// Mirrors the compiler's own resolution (`wavelet::witdep::resolve_dep`): the
/// LSP and the build therefore agree on which packages and functions exist.
/// Silently yields nothing when the path is unknown, no `wit/deps` exists, or a
/// package isn't present there — completion stays best-effort, never an error.
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
        // `Ok(None)` = package not vendored here; `Err` = a parse failure in
        // `wit/deps`. Either way we just contribute no completions for it.
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

/// A `.wvl` source lives in a project's `src/`; its external WIT dependencies
/// sit beside that in `<project-root>/wit/deps`. Derive that directory from the
/// document path, matching `build.rs`'s `wit_deps_dir`. Returns `None` for a
/// path with no parent chain to a project root.
fn wit_deps_dir(file: &Path) -> Option<PathBuf> {
    let src_dir = file.parent()?;
    let root = src_dir.parent().unwrap_or(src_dir);
    Some(root.join("wit").join("deps"))
}

/// Hover: explain special forms and builtins, and surface a definition's `///`
/// doc comment when the cursor is on the name it defines.
pub fn hover(text: &str, index: &LineIndex, offset: usize) -> Option<Hover> {
    let (arena, roots) = wavelet::read_file(text).ok()?;
    let node_id = smallest_node_at(&arena, offset)?;
    let name = match arena.node(node_id) {
        Node::Sym(s) | Node::Qsym(_, s) => s.clone(),
        _ => return None,
    };

    // A `name-MACRO` head is a special form; show its TitleCase blurb.
    if name.ends_with("-MACRO") {
        if let Some((title, detail)) = SPECIAL_FORMS
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
    }

    // A reference to a name defined in this file: show its doc comment.
    if let Some(doc) = definition_doc(&arena, &roots, &name) {
        return Some(markdown_hover(
            format!("**{name}**\n\n{doc}"),
            arena.span(node_id),
            text,
            index,
        ));
    }

    // A builtin reference.
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

/// Document symbols: top-level `Def`, `DefType`, and `DefMacro` forms.
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

// ---- form-tree helpers ---------------------------------------------------

/// `(name, kind, call-node-id)` for every top-level definition form.
fn definitions(arena: &Arena, roots: &[NodeId]) -> Vec<(String, SymbolKind, NodeId)> {
    let mut out = Vec::new();
    for &root in roots {
        let Node::Call(head, _) = arena.node(root) else { continue };
        let kind = match sym_name(arena, *head).as_deref() {
            Some("def-MACRO") => SymbolKind::FUNCTION,
            Some("def-type-MACRO") => SymbolKind::STRUCT,
            Some("def-macro-MACRO") => SymbolKind::OPERATOR,
            _ => continue,
        };
        if let Some(name) = def_name_node(arena, root).and_then(|n| sym_name(arena, n)) {
            out.push((name, kind, root));
        }
    }
    out
}

/// The node naming a definition: the first argument of its payload.
fn def_name_node(arena: &Arena, call_id: NodeId) -> Option<NodeId> {
    let Node::Call(_, payload) = arena.node(call_id) else { return None };
    Some(match arena.node(*payload) {
        Node::Tup(items) => *items.first()?,
        _ => *payload,
    })
}

fn definition_doc(arena: &Arena, roots: &[NodeId], name: &str) -> Option<String> {
    for &root in roots {
        if matches!(arena.node(root), Node::Call(..))
            && def_name_node(arena, root).and_then(|n| sym_name(arena, n)).as_deref()
                == Some(name)
        {
            return arena.doc(root).map(str::to_string);
        }
    }
    None
}

fn sym_name(arena: &Arena, id: NodeId) -> Option<String> {
    match arena.node(id) {
        Node::Sym(s) | Node::Qsym(_, s) => Some(s.clone()),
        _ => None,
    }
}

/// The narrowest node whose span contains `offset`.
fn smallest_node_at(arena: &Arena, offset: usize) -> Option<NodeId> {
    let mut best: Option<(NodeId, u32)> = None;
    for id in 0..arena.nodes.len() as NodeId {
        let (s, e) = arena.span(id);
        if (s as usize) <= offset && offset < (e as usize) {
            let width = e - s;
            if best.map_or(true, |(_, w)| width < w) {
                best = Some((id, width));
            }
        }
    }
    best.map(|(id, _)| id)
}

// ---- position helpers ----------------------------------------------------

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
        completions(text, path).into_iter().map(|c| c.label).collect()
    }

    /// The WASI builtins and the `Target` form were removed from the language
    /// (the WASI-decoupling work); completion must no longer surface them.
    #[test]
    fn removed_builtins_and_target_are_gone() {
        let got = labels("", None);
        for dead in ["Target", "print", "println", "read-line", "args", "env"] {
            assert!(
                !got.iter().any(|l| l == dead),
                "completion still offers removed `{dead}`: {got:?}"
            );
        }
        // Sanity: a surviving special form and builtin are still offered.
        assert!(got.iter().any(|l| l == "Import"));
        assert!(got.iter().any(|l| l == "map"));
    }

    /// An `Import` of a package vendored under the project's `wit/deps` offers
    /// that package's functions, labelled `alias/func` the way source calls them.
    #[test]
    fn import_resolves_functions_from_wit_deps() {
        let dir = std::env::temp_dir()
            .join(format!("wavelet-lsp-analysis-{}", std::process::id()));
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

        let file = src.join("main.wvl");
        let text = "Package \"demo:app@0.1.0\"\n\
                    Import {pkg: \"acme:greet/api\" as: greeting}\n";

        let got = labels(text, Some(&file));
        assert!(
            got.iter().any(|l| l == "greeting/greet"),
            "expected `greeting/greet` from wit/deps import: {got:?}"
        );

        // Without a path (in-memory buffer) the import completions are skipped,
        // but the rest is unaffected.
        let no_path = labels(text, None);
        assert!(!no_path.iter().any(|l| l == "greeting/greet"));
        assert!(no_path.iter().any(|l| l == "Import"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
