use std::collections::HashMap;

use crate::form::{Arena, Node, NodeId};
use crate::lexer::{lex, ReadError, Tok, Token};

/// Arity table for TitleCase heads: core special forms plus macros
/// registered by `DefMacro` as the reader moves top to bottom (§2.4).
pub struct MacroTable {
    map: HashMap<String, usize>,
}

impl MacroTable {
    pub fn core() -> Self {
        let mut map = HashMap::new();
        for (name, arity) in [
            ("package-MACRO", 1),
            ("import-MACRO", 1),
            ("export-MACRO", 1),
            ("def-type-MACRO", 2),
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
            ("def-macro-MACRO", 3),
            ("the-MACRO", 2),
        ] {
            map.insert(name.to_string(), arity);
        }
        Self { map }
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.map.get(name).copied()
    }

    pub fn register(&mut self, name: String, arity: usize) {
        self.map.insert(name, arity);
    }
}

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

    fn parse_form(&mut self) -> Result<NodeId, ReadError> {
        // leading `///` lines attach to the form that follows (§2.1)
        let mut doc: Option<String> = None;
        while let Some((Tok::Doc(_), _)) = self.peek() {
            let (Tok::Doc(text), _) = self.next()? else { unreachable!() };
            match &mut doc {
                Some(d) => {
                    d.push('\n');
                    d.push_str(&text);
                }
                None => doc = Some(text),
            }
        }
        let id = self.parse_form_inner()?;
        if let Some(d) = doc {
            self.arena.set_doc(id, d);
        }
        Ok(id)
    }

    fn parse_form_inner(&mut self) -> Result<NodeId, ReadError> {
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
                let items = self.parse_until_bracket()?;
                Ok(self.arena.add(Node::Lst(items), sp))
            }
            Tok::LBrace => self.parse_braces(span),
            Tok::RParen | Tok::RBracket | Tok::RBrace => {
                self.err("unexpected closing delimiter", span.start)
            }
            Tok::Colon => self.err("`:` is only valid inside a record", span.start),
            Tok::Doc(_) => self.err("`///` doc comment must precede a form", span.start),
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
        let name = match self.arena.node(head) {
            Node::Sym(s) | Node::Qsym(_, s) => s.clone(),
            _ => unreachable!(),
        };
        let arity = match self.macros.arity(&name) {
            Some(a) => a,
            None => {
                return self.err(
                    format!("unknown macro `{name}` (macros must be in scope before use)"),
                    span.start,
                )
            }
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

    fn parse_until_bracket(&mut self) -> Result<Vec<NodeId>, ReadError> {
        self.parse_until(Tok::RBracket)
    }

    fn parse_until(&mut self, close: Tok) -> Result<Vec<NodeId>, ReadError> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Some((t, _)) if *t == close => {
                    self.pos += 1;
                    return Ok(items);
                }
                Some(_) => items.push(self.parse_form()?),
                None => return Err(self.eof_err()),
            }
        }
    }

    /// `{…}`: record if the first name is followed by `:`, otherwise flags.
    fn parse_braces(&mut self, span: Token) -> Result<NodeId, ReadError> {
        let sp = (span.start, span.end);
        if let Some((Tok::RBrace, _)) = self.peek() {
            self.pos += 1;
            return Ok(self.arena.add(Node::Flg(vec![]), sp));
        }
        let is_record = matches!(self.toks.get(self.pos + 1), Some((Tok::Colon, _)));
        if is_record {
            let mut fields = Vec::new();
            loop {
                match self.next()? {
                    (Tok::RBrace, _) => return Ok(self.arena.add(Node::Rec(fields), sp)),
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
                    (Tok::RBrace, _) => return Ok(self.arena.add(Node::Flg(names), sp)),
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
        // tuple `Tup[def-macro-MACRO, name, params, body]`.
        let Node::Tup(items) = self.arena.node(id) else { return };
        if items.len() != 4 {
            return;
        }
        let Node::Sym(h) = self.arena.node(items[0]) else { return };
        if h != "def-macro-MACRO" {
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
