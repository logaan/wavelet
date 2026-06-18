use std::collections::HashMap;

use crate::form::{Arena, Node, NodeId};
use crate::lexer::{lex, ReadError, Tok, Token};

/// Where a macro arity registration came from, so the table can tell a benign
/// re-registration (same origin) from a genuine *collision* (two different
/// origins claiming one bare name). Core special forms and file-local
/// `DefMacro`s share the [`Origin::Local`] namespace; each `macros: true` import
/// is its own [`Origin::Import`] keyed by the import's `as:` alias (§6.3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Origin {
    /// A core special form or a file-local `DefMacro`.
    Local,
    /// A macro exported by the import bound to this alias.
    Import(String),
}

/// Arity table for TitleCase heads: core special forms plus macros registered by
/// `DefMacro` or by `macros: true` imports as the reader moves top to bottom
/// (§2.4, §6.3).
///
/// Bare `Name` lookups consult [`MacroTable::arity`]; qualified `Alias/Name`
/// lookups consult [`MacroTable::arity_qualified`]. When two different origins
/// register the same bare name the bare name becomes *ambiguous* — bare use is a
/// read-time error pointing the author at aliasing/qualifying — while each
/// origin's qualified key keeps resolving (collision policy (a), see the Step 8
/// notes).
pub struct MacroTable {
    /// Bare name → (arity, the single origin that owns it). A name present here
    /// resolves unambiguously by its bare spelling.
    map: HashMap<String, (usize, Origin)>,
    /// `(alias, name)` → arity for every import-provided macro, registered
    /// regardless of collisions so a qualified `Alias/Name` head always resolves.
    qualified: HashMap<(String, String), usize>,
    /// Bare name → the set of import aliases that provide it, recorded once a
    /// bare name has been claimed by more than one origin. A name in this map is
    /// dropped from `map`, so bare use errors actionably.
    ambiguous: HashMap<String, Vec<String>>,
}

impl MacroTable {
    pub fn core() -> Self {
        let mut map = HashMap::new();
        for (name, arity) in [
            ("package-MACRO", 1),
            ("import-MACRO", 1),
            ("export-MACRO", 1),
            ("deftype-MACRO", 2),
            ("def-MACRO", 2),
            ("fn-MACRO", 2),
            ("if-MACRO", 3),
            ("let-MACRO", 2),
            ("do-MACRO", 1),
            ("match-MACRO", 2),
            ("quote-MACRO", 1),
            ("quasi-MACRO", 1),
            ("unquote-MACRO", 1),
            ("splice-MACRO", 1),
            ("defmacro-MACRO", 3),
            ("the-MACRO", 2),
        ] {
            map.insert(name.to_string(), (arity, Origin::Local));
        }
        Self { map, qualified: HashMap::new(), ambiguous: HashMap::new() }
    }

    /// Arity of an unambiguous bare TitleCase head. Returns `None` for an unknown
    /// name *and* for an ambiguous one — the read path tells the two apart via
    /// [`MacroTable::is_ambiguous`] to produce the right error.
    pub fn arity(&self, name: &str) -> Option<usize> {
        self.map.get(name).map(|(a, _)| *a)
    }

    /// Arity of a qualified `Alias/Name` head, looked up by the import alias and
    /// the (already `-MACRO`-suffixed) name. Independent of bare-name collisions.
    pub fn arity_qualified(&self, alias: &str, name: &str) -> Option<usize> {
        self.qualified
            .get(&(alias.to_string(), name.to_string()))
            .copied()
    }

    /// Whether a bare name has been claimed by more than one origin (so a bare
    /// use is an error). If so, returns the aliases that provide it.
    pub fn is_ambiguous(&self, name: &str) -> Option<&[String]> {
        self.ambiguous.get(name).map(|v| v.as_slice())
    }

    /// Register a core special form or file-local `DefMacro` under its bare name
    /// (origin [`Origin::Local`]). A local `DefMacro` that collides with an
    /// already-registered macro of a *different* origin makes the bare name
    /// ambiguous, exactly like two imports colliding.
    pub fn register(&mut self, name: String, arity: usize) {
        self.register_with_origin(name, arity, Origin::Local);
    }

    /// Register an import-provided macro: always under its qualified `(alias,
    /// name)` key, and — subject to collision detection — under its bare name.
    pub fn register_foreign(&mut self, alias: &str, name: String, arity: usize) {
        self.qualified
            .insert((alias.to_string(), name.clone()), arity);
        self.register_with_origin(name, arity, Origin::Import(alias.to_string()));
    }

    /// Shared bare-name registration with collision tracking. Re-registering a
    /// name under the *same* origin (e.g. the same package resolved twice) is a
    /// harmless update; a *different* origin makes the bare name ambiguous.
    fn register_with_origin(&mut self, name: String, arity: usize, origin: Origin) {
        // Already ambiguous: just record this origin's alias (if an import) and
        // keep the bare name out of `map`.
        if let Some(aliases) = self.ambiguous.get_mut(&name) {
            if let Origin::Import(alias) = &origin {
                if !aliases.contains(alias) {
                    aliases.push(alias.clone());
                }
            }
            return;
        }
        match self.map.get(&name) {
            Some((_, existing)) if *existing == origin => {
                // Same origin re-registering — update the arity in place.
                self.map.insert(name, (arity, origin));
            }
            Some((_, existing)) => {
                // Genuine collision: drop the bare name and mark it ambiguous,
                // collecting both contributing import aliases (a local origin
                // contributes no alias but still makes the name ambiguous).
                let existing = existing.clone();
                self.map.remove(&name);
                let mut aliases = Vec::new();
                if let Origin::Import(a) = &existing {
                    aliases.push(a.clone());
                }
                if let Origin::Import(a) = &origin {
                    if !aliases.contains(a) {
                        aliases.push(a.clone());
                    }
                }
                self.ambiguous.insert(name, aliases);
            }
            None => {
                self.map.insert(name, (arity, origin));
            }
        }
    }
}

/// A hook the reader runs after each top-level form is parsed, given the form's
/// node id, so a caller can register *foreign* macro arities into the table as
/// the reader moves top-to-bottom — exactly when a local `DefMacro` would be
/// registered. This is how `Import {… macros: true}` arities become known
/// (§6.3): the native compiler supplies a hook that resolves the import's macro
/// component and registers its `manifest()` pairs (see
/// [`crate::macrodep::register_macro_imports`]).
///
/// The hook is a plain closure over reader/`form` types only, so `reader.rs`
/// keeps compiling for the `wasm32` playground — where there is no component
/// runtime and thus no hook is supplied, and foreign registration is simply
/// absent. An `Err` returned by the hook aborts the read with that error (e.g.
/// a macro component that fails to instantiate, tied to the import's span).
pub type FormHook<'a> =
    dyn FnMut(&Arena, NodeId, &mut MacroTable) -> Result<(), ReadError> + 'a;

pub fn read_file(src: &str) -> Result<(Arena, Vec<NodeId>), ReadError> {
    let mut macros = MacroTable::core();
    read_with(src, &mut macros)
}

/// Like [`read_file`] but with a caller-owned arity table, so `DefMacro`
/// registrations persist across inputs (used by the REPL).
pub fn read_with(
    src: &str,
    macros: &mut MacroTable,
) -> Result<(Arena, Vec<NodeId>), ReadError> {
    read_with_hook(src, macros, None)
}

/// The full read entry point: a caller-owned arity table plus an optional
/// per-form [`FormHook`]. After each top-level form is parsed (and after the
/// built-in local-`DefMacro` registration), the hook — if any — runs against
/// that form, letting the native compiler register foreign macro arities from a
/// `macros: true` import *before* later forms that use those macros are read.
/// Passing `None` reproduces [`read_with`] exactly, so the wasm playground and
/// the REPL are unchanged.
pub fn read_with_hook(
    src: &str,
    macros: &mut MacroTable,
    mut hook: Option<&mut FormHook<'_>>,
) -> Result<(Arena, Vec<NodeId>), ReadError> {
    let toks = lex(src)?;
    let mut p = Parser {
        toks,
        pos: 0,
        arena: Arena::new(),
        macros: std::mem::replace(macros, MacroTable::core()),
    };
    let mut roots = Vec::new();
    let result = (|| {
        while !p.at_end() {
            let id = p.parse_form()?;
            p.register_if_def_macro(id);
            if let Some(hook) = hook.as_deref_mut() {
                hook(&p.arena, id, &mut p.macros)?;
            }
            roots.push(id);
        }
        Ok(())
    })();
    *macros = p.macros;
    result.map(|()| (p.arena, roots))
}

struct Parser {
    toks: Vec<(Tok, Token)>,
    pos: usize,
    arena: Arena,
    macros: MacroTable,
}

impl Parser {
    fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    fn peek(&self) -> Option<&(Tok, Token)> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Result<(Tok, Token), ReadError> {
        match self.toks.get(self.pos) {
            Some(t) => {
                self.pos += 1;
                Ok(t.clone())
            }
            None => Err(self.eof_err()),
        }
    }

    fn eof_err(&self) -> ReadError {
        let at = self.toks.last().map(|(_, s)| s.end).unwrap_or(0);
        ReadError { msg: "unexpected end of input".into(), at }
    }

    fn err<T>(&self, msg: impl Into<String>, at: u32) -> Result<T, ReadError> {
        Err(ReadError { msg: msg.into(), at })
    }

    /// §2.2: only `(` attaches to an identifier to form a call. An attached
    /// `[` or `{` (no whitespace) is a read error — the list/record call sugar
    /// was removed; it points the user at the new `name([…])` / `name({…})`
    /// spelling. Free-standing `[…]`/`{…}` (with whitespace) are unaffected.
    fn attached_paren(&self, end: u32) -> Result<bool, ReadError> {
        match self.peek() {
            Some((Tok::LParen, span)) if span.start == end => Ok(true),
            Some((Tok::LBracket, span)) if span.start == end => self.err(
                "list call sugar was removed: write `name([...])` instead of `name[...]`",
                span.start,
            ),
            Some((Tok::LBrace, span)) if span.start == end => self.err(
                "record call sugar was removed: write `name({...})` instead of `name{...}`",
                span.start,
            ),
            _ => Ok(false),
        }
    }

    /// Parse one form, then fold any attached call chain onto it (§2.5):
    /// `recv.name(args)` reads as the call `(name, recv, …args)`, left-to-right.
    fn parse_form(&mut self) -> Result<NodeId, ReadError> {
        let recv = self.parse_primary()?;
        self.maybe_chain(recv)
    }

    /// Parse a single primary form (atom, name, call, parens, list, record,
    /// macro form) — the chain receiver, before any `.name(...)` suffix.
    fn parse_primary(&mut self) -> Result<NodeId, ReadError> {
        let (tok, span) = self.next()?;
        let sp = (span.start, span.end);
        match tok {
            Tok::Bool(b) => Ok(self.arena.add(Node::Bool(b), sp)),
            Tok::Int(n) => Ok(self.arena.add(Node::Int(n), sp)),
            Tok::Dec(f) => Ok(self.arena.add(Node::Dec(f), sp)),
            Tok::Char(c) => Ok(self.arena.add(Node::Char(c), sp)),
            Tok::Str(s) => Ok(self.arena.add(Node::Str(s), sp)),
            Tok::Ident(name) => {
                let head = self.arena.add(Node::Sym(name), sp);
                self.maybe_call(head, span)
            }
            Tok::QIdent(alias, name, is_title) => {
                if is_title {
                    let head = self.arena.add(Node::Qsym(alias, name), sp);
                    self.title_form(head, span)
                } else {
                    let head = self.arena.add(Node::Qsym(alias, name), sp);
                    self.maybe_call(head, span)
                }
            }
            Tok::Title(name) => {
                let head = self.arena.add(Node::Sym(name), sp);
                self.title_form(head, span)
            }
            Tok::LParen => self.parse_parens(span),
            Tok::LBracket => {
                let (items, end) = self.parse_until_bracket()?;
                Ok(self.arena.add(Node::Lst(items), (span.start, end)))
            }
            Tok::LBrace => self.parse_braces(span),
            Tok::RParen | Tok::RBracket | Tok::RBrace => {
                self.err("unexpected closing delimiter", span.start)
            }
            Tok::Colon => self.err("`:` is only valid inside a record", span.start),
            Tok::Dot => self.err(
                "`.` chains a call onto the form immediately before it (no leading space)",
                span.start,
            ),
        }
    }

    /// §2.5: fold a chain of attached `.name(args)` calls onto `recv`, turning
    /// the receiver into the call's first argument. `recv.name(a b)` reads as
    /// `(name, recv, a, b)`; chains nest left-to-right. The `.`, the name, and
    /// the `(` must each abut the preceding token with no whitespace — this is
    /// pure reader rewriting, not method dispatch.
    fn maybe_chain(&mut self, mut recv: NodeId) -> Result<NodeId, ReadError> {
        loop {
            let (recv_start, recv_end) = self.arena.span(recv);
            match self.peek() {
                Some((Tok::Dot, span)) if span.start == recv_end => {}
                _ => return Ok(recv),
            }
            let (_, dot) = self.next()?; // the attached `.`
            let (tok, name_span) = self.next()?;
            if name_span.start != dot.end {
                return self.err("a chained call name must immediately follow `.`", name_span.start);
            }
            let nsp = (name_span.start, name_span.end);
            let head = match tok {
                Tok::Ident(name) => self.arena.add(Node::Sym(name), nsp),
                Tok::QIdent(alias, name, _) => self.arena.add(Node::Qsym(alias, name), nsp),
                _ => return self.err("expected a call name after `.`", name_span.start),
            };
            if !self.attached_paren(name_span.end)? {
                return self.err("a chained call needs `(...)` after the name", name_span.end);
            }
            let (items, end) = self.parse_paren_items()?;
            // `(name, recv, …items)` — the receiver spliced in as the first arg.
            let mut all = Vec::with_capacity(items.len() + 2);
            all.push(head);
            all.push(recv);
            all.extend(items);
            recv = self.arena.add(Node::Tup(all), (recv_start, end));
        }
    }

    /// Identifier head: a call (`Tup[head, …items]`) if `(` is attached,
    /// otherwise a plain variable reference.
    fn maybe_call(&mut self, head: NodeId, span: Token) -> Result<NodeId, ReadError> {
        if self.attached_paren(span.end)? {
            let (items, end) = self.parse_paren_items()?;
            Ok(self.call_tup(head, items, span.start, end))
        } else {
            Ok(head)
        }
    }

    /// TitleCase head: explicit payload overrides arity-driven reading (§2.4).
    /// Either way the head is prepended into a flat `Tup`.
    fn title_form(&mut self, head: NodeId, span: Token) -> Result<NodeId, ReadError> {
        if self.attached_paren(span.end)? {
            let (items, end) = self.parse_paren_items()?;
            return Ok(self.call_tup(head, items, span.start, end));
        }
        // A qualified head (`Alias/Name`) resolves arity via the import bound to
        // its alias; a bare head resolves via the unambiguous bare table (§2.4,
        // §6.3). An ambiguous bare name is a distinct, actionable error.
        let arity = match self.arena.node(head).clone() {
            Node::Qsym(alias, name) => match self.macros.arity_qualified(&alias, &name) {
                Some(a) => a,
                None => {
                    let pretty = name.trim_end_matches("-MACRO");
                    return self.err(
                        format!(
                            "unknown qualified macro `{alias}/{pretty}` \
                             (no `macros: true` import aliased `{alias}` provides `{pretty}`)"
                        ),
                        span.start,
                    );
                }
            },
            Node::Sym(name) => {
                if let Some(aliases) = self.macros.is_ambiguous(&name) {
                    let pretty = name.trim_end_matches("-MACRO");
                    let hint = if aliases.is_empty() {
                        format!("qualify it or alias the conflicting import")
                    } else {
                        let qualified: Vec<String> =
                            aliases.iter().map(|a| format!("`{a}/{pretty}`")).collect();
                        format!(
                            "qualify the use ({}) or alias the imports with `as:`",
                            qualified.join(" / ")
                        )
                    };
                    return self.err(
                        format!("ambiguous macro `{pretty}` is provided by more than one import — {hint}"),
                        span.start,
                    );
                }
                match self.macros.arity(&name) {
                    Some(a) => a,
                    None => {
                        return self.err(
                            format!(
                                "unknown macro `{name}` (macros must be in scope before use)"
                            ),
                            span.start,
                        );
                    }
                }
            }
            _ => unreachable!(),
        };
        let mut items = Vec::with_capacity(arity);
        for _ in 0..arity {
            items.push(self.parse_form()?);
        }
        let end = items
            .last()
            .map(|&n| self.arena.span(n).1)
            .unwrap_or(span.end);
        Ok(self.call_tup(head, items, span.start, end))
    }

    /// Build a call form `Tup[head, …items]` (the head spliced in front).
    fn call_tup(&mut self, head: NodeId, items: Vec<NodeId>, start: u32, end: u32) -> NodeId {
        let mut all = Vec::with_capacity(items.len() + 1);
        all.push(head);
        all.extend(items);
        self.arena.add(Node::Tup(all), (start, end))
    }

    /// Consume an attached `(`, parse items up to `)`, return `(items, close_end)`.
    fn parse_paren_items(&mut self) -> Result<(Vec<NodeId>, u32), ReadError> {
        self.next()?; // the attached `(`
        self.parse_paren_body()
    }

    /// Free-standing parens: every `( … )` is a tuple/call form.
    /// `()` ⇒ `Tup[]` (errors only at eval time); `(a)` ⇒ `Tup[a]` (0-arg call,
    /// not transparent grouping); `(a b …)` ⇒ `Tup[a, b, …]`.
    fn parse_parens(&mut self, span: Token) -> Result<NodeId, ReadError> {
        let (items, end) = self.parse_paren_body()?;
        Ok(self.arena.add(Node::Tup(items), (span.start, end)))
    }

    /// Parse items up to the closing `)` (already past the `(`), returning the
    /// items and the closing paren's end offset.
    fn parse_paren_body(&mut self) -> Result<(Vec<NodeId>, u32), ReadError> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Some((Tok::RParen, span)) => {
                    let end = span.end;
                    self.pos += 1;
                    return Ok((items, end));
                }
                Some(_) => items.push(self.parse_form()?),
                None => return Err(self.eof_err()),
            }
        }
    }

    /// Parse list items up to `]`, returning the items and the `]`'s end offset
    /// (so the `Lst` node spans the whole literal, e.g. for chain adjacency).
    fn parse_until_bracket(&mut self) -> Result<(Vec<NodeId>, u32), ReadError> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Some((Tok::RBracket, span)) => {
                    let end = span.end;
                    self.pos += 1;
                    return Ok((items, end));
                }
                Some(_) => items.push(self.parse_form()?),
                None => return Err(self.eof_err()),
            }
        }
    }

    /// `{…}`: record if the first name is followed by `:`, otherwise flags.
    fn parse_braces(&mut self, span: Token) -> Result<NodeId, ReadError> {
        let start = span.start;
        if let Some((Tok::RBrace, close)) = self.peek() {
            let sp = (start, close.end);
            self.pos += 1;
            return Ok(self.arena.add(Node::Flg(vec![]), sp));
        }
        let is_record = matches!(self.toks.get(self.pos + 1), Some((Tok::Colon, _)));
        if is_record {
            let mut fields = Vec::new();
            loop {
                match self.next()? {
                    (Tok::RBrace, close) => {
                        return Ok(self.arena.add(Node::Rec(fields), (start, close.end)))
                    }
                    (Tok::Ident(name), _) => {
                        match self.next()? {
                            (Tok::Colon, _) => {}
                            (_, s) => return self.err("expected `:` after record field name", s.start),
                        }
                        let value = self.parse_form()?;
                        fields.push((name, value));
                    }
                    (_, s) => return self.err("expected a record field name", s.start),
                }
            }
        } else {
            let mut names = Vec::new();
            loop {
                match self.next()? {
                    (Tok::RBrace, close) => {
                        return Ok(self.arena.add(Node::Flg(names), (start, close.end)))
                    }
                    (Tok::Ident(name), _) => names.push(name),
                    (_, s) => return self.err("expected a flag name", s.start),
                }
            }
        }
    }

    /// After a top-level `DefMacro name {params} body`, register the macro's
    /// arity so later TitleCase uses in this file can be read (§2.4).
    fn register_if_def_macro(&mut self, id: NodeId) {
        // A top-level `DefMacro name {params} body` reads as the 4-element
        // tuple `Tup[defmacro-MACRO, name, params, body]`.
        let Node::Tup(items) = self.arena.node(id) else { return };
        if items.len() != 4 {
            return;
        }
        let Node::Sym(h) = self.arena.node(items[0]) else { return };
        if h != "defmacro-MACRO" {
            return;
        }
        let Node::Sym(name) = self.arena.node(items[1]) else { return };
        let arity = match self.arena.node(items[2]) {
            Node::Flg(names) => names.len(),
            Node::Rec(fields) => fields.len(),
            _ => return,
        };
        self.macros.register(format!("{name}-MACRO"), arity);
    }
}

#[cfg(test)]
mod macro_table_tests {
    use super::*;

    #[test]
    fn single_foreign_macro_resolves_bare_and_qualified() {
        let mut t = MacroTable::core();
        t.register_foreign("dsl", "unless-MACRO".into(), 2);
        assert_eq!(t.arity("unless-MACRO"), Some(2));
        assert_eq!(t.arity_qualified("dsl", "unless-MACRO"), Some(2));
        assert!(t.is_ambiguous("unless-MACRO").is_none());
    }

    #[test]
    fn two_imports_same_name_make_bare_ambiguous_qualified_ok() {
        let mut t = MacroTable::core();
        t.register_foreign("dsl", "unless-MACRO".into(), 2);
        t.register_foreign("web", "unless-MACRO".into(), 2);
        // Bare name now ambiguous, both qualified keys resolve.
        assert_eq!(t.arity("unless-MACRO"), None);
        let aliases = t.is_ambiguous("unless-MACRO").expect("ambiguous");
        assert!(aliases.contains(&"dsl".to_string()));
        assert!(aliases.contains(&"web".to_string()));
        assert_eq!(t.arity_qualified("dsl", "unless-MACRO"), Some(2));
        assert_eq!(t.arity_qualified("web", "unless-MACRO"), Some(2));
    }

    #[test]
    fn local_defmacro_collides_with_import() {
        let mut t = MacroTable::core();
        t.register_foreign("dsl", "thing-MACRO".into(), 1);
        // A local DefMacro of the same bare name collides → ambiguous.
        t.register("thing-MACRO".into(), 1);
        assert_eq!(t.arity("thing-MACRO"), None);
        assert!(t.is_ambiguous("thing-MACRO").is_some());
        // The import's qualified key still resolves.
        assert_eq!(t.arity_qualified("dsl", "thing-MACRO"), Some(1));
    }

    #[test]
    fn same_package_two_aliases_via_repeated_register_is_collision() {
        // The resolver registers the same package's macros under each alias;
        // the second alias collides on the bare name.
        let mut t = MacroTable::core();
        t.register_foreign("dsl", "identity-MACRO".into(), 1);
        t.register_foreign("html", "identity-MACRO".into(), 1);
        assert!(t.is_ambiguous("identity-MACRO").is_some());
        assert_eq!(t.arity_qualified("html", "identity-MACRO"), Some(1));
    }

    #[test]
    fn core_special_forms_unaffected_by_foreign_macros() {
        let mut t = MacroTable::core();
        t.register_foreign("dsl", "unless-MACRO".into(), 2);
        // Core forms stay unambiguous and resolvable by their bare names.
        assert_eq!(t.arity("if-MACRO"), Some(3));
        assert!(t.is_ambiguous("if-MACRO").is_none());
    }
}
