use std::collections::HashMap;

use crate::form::{Arena, Node, NodeId};
use crate::lexer::{ReadError, Tok, Token, lex};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Origin {
    Local,

    Import(String),
}

pub struct MacroTable {
    map: HashMap<String, (usize, Origin)>,

    qualified: HashMap<(String, String), usize>,

    ambiguous: HashMap<String, Vec<String>>,
}

impl MacroTable {
    pub fn core() -> Self {
        let mut map = HashMap::new();
        for (name, arity) in [
            ("package-MACRO", 1),
            ("import-MACRO", 1),
            ("instantiate-MACRO", 1),
            ("deffunctor-MACRO", 3),
            ("export-MACRO", 1),
            ("deftype-MACRO", 2),
            ("defresource-MACRO", 2),
            ("static-MACRO", 1),
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
            ("derive-MACRO", 2),
        ] {
            map.insert(name.to_string(), (arity, Origin::Local));
        }
        Self {
            map,
            qualified: HashMap::new(),
            ambiguous: HashMap::new(),
        }
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.map.get(name).map(|(a, _)| *a)
    }

    pub fn arity_qualified(&self, alias: &str, name: &str) -> Option<usize> {
        self.qualified
            .get(&(alias.to_string(), name.to_string()))
            .copied()
    }

    pub fn is_ambiguous(&self, name: &str) -> Option<&[String]> {
        self.ambiguous.get(name).map(|v| v.as_slice())
    }

    pub fn register(&mut self, name: String, arity: usize) {
        self.register_with_origin(name, arity, Origin::Local);
    }

    pub fn register_foreign(&mut self, alias: &str, name: String, arity: usize) {
        self.qualified
            .insert((alias.to_string(), name.clone()), arity);
        self.register_with_origin(
            format!("{}-{name}", alias.to_ascii_lowercase()),
            arity,
            Origin::Import(alias.to_string()),
        );
        self.register_with_origin(name, arity, Origin::Import(alias.to_string()));
    }

    fn register_with_origin(&mut self, name: String, arity: usize, origin: Origin) {
        if let Some(aliases) = self.ambiguous.get_mut(&name) {
            if let Origin::Import(alias) = &origin
                && !aliases.contains(alias)
            {
                aliases.push(alias.clone());
            }
            return;
        }
        match self.map.get(&name) {
            Some((_, existing)) if *existing == origin => {
                self.map.insert(name, (arity, origin));
            }
            Some((_, existing)) => {
                let existing = existing.clone();
                self.map.remove(&name);
                let mut aliases = Vec::new();
                if let Origin::Import(a) = &existing {
                    aliases.push(a.clone());
                }
                if let Origin::Import(a) = &origin
                    && !aliases.contains(a)
                {
                    aliases.push(a.clone());
                }
                self.ambiguous.insert(name, aliases);
            }
            None => {
                self.map.insert(name, (arity, origin));
            }
        }
    }
}

pub type FormHook<'a> = dyn FnMut(&Arena, NodeId, &mut MacroTable) -> Result<(), ReadError> + 'a;

fn title_flag_name(macro_name: &str) -> String {
    let bare = macro_name.trim_end_matches("-MACRO");
    let mut chars = bare.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

pub fn read_file(src: &str) -> Result<(Arena, Vec<NodeId>), ReadError> {
    let mut macros = MacroTable::core();
    read_with(src, &mut macros)
}

pub fn read_with(src: &str, macros: &mut MacroTable) -> Result<(Arena, Vec<NodeId>), ReadError> {
    read_with_hook(src, macros, None)
}

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
        allow_title_flags: false,
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
    result?;
    crate::templates::instantiate(p.arena, roots)
}

struct Parser {
    toks: Vec<(Tok, Token)>,
    pos: usize,
    arena: Arena,
    macros: MacroTable,

    allow_title_flags: bool,
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
        ReadError {
            msg: "unexpected end of input".into(),
            at,
        }
    }

    fn err<T>(&self, msg: impl Into<String>, at: u32) -> Result<T, ReadError> {
        Err(ReadError {
            msg: msg.into(),
            at,
        })
    }

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
        let recv = self.parse_primary()?;
        self.maybe_chain(recv)
    }

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

    fn maybe_chain(&mut self, mut recv: NodeId) -> Result<NodeId, ReadError> {
        loop {
            let (recv_start, recv_end) = self.arena.span(recv);
            match self.peek() {
                Some((Tok::Dot, span)) if span.start == recv_end => {}
                _ => return Ok(recv),
            }
            let (_, dot) = self.next()?;
            let (tok, name_span) = self.next()?;
            if name_span.start != dot.end {
                return self.err(
                    "a chained call name must immediately follow `.`",
                    name_span.start,
                );
            }
            let nsp = (name_span.start, name_span.end);
            let head = match tok {
                Tok::Ident(name) => self.arena.add(Node::Sym(name), nsp),
                _ => return self.err("expected a call name after `.`", name_span.start),
            };
            if !self.attached_paren(name_span.end)? {
                return self.err("a chained call needs `(...)` after the name", name_span.end);
            }
            let (items, end) = self.parse_paren_items()?;

            let mut all = Vec::with_capacity(items.len() + 2);
            all.push(head);
            all.push(recv);
            all.extend(items);
            recv = self.arena.add(Node::Tup(all), (recv_start, end));
        }
    }

    fn maybe_call(&mut self, head: NodeId, span: Token) -> Result<NodeId, ReadError> {
        if self.attached_paren(span.end)? {
            let (items, end) = self.parse_paren_items()?;
            Ok(self.call_tup(head, items, span.start, end))
        } else {
            Ok(head)
        }
    }

    fn title_form(&mut self, head: NodeId, span: Token) -> Result<NodeId, ReadError> {
        if self.attached_paren(span.end)? {
            let (items, end) = self.parse_paren_items()?;
            return Ok(self.call_tup(head, items, span.start, end));
        }

        let arity = match self.arena.node(head).clone() {
            Node::Sym(name) => {
                if let Some(aliases) = self.macros.is_ambiguous(&name) {
                    let pretty = name.trim_end_matches("-MACRO");
                    let hint = if aliases.is_empty() {
                        "qualify it or alias the conflicting import".to_string()
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
                            format!("unknown macro `{name}` (macros must be in scope before use)"),
                            span.start,
                        );
                    }
                }
            }
            _ => unreachable!(),
        };
        let derive = matches!(self.arena.node(head), Node::Sym(n) if n == "derive-MACRO");
        let mut items = Vec::with_capacity(arity);
        for i in 0..arity {
            self.allow_title_flags = derive && i == 0;
            items.push(self.parse_form()?);
        }
        self.allow_title_flags = false;
        let end = items
            .last()
            .map(|&n| self.arena.span(n).1)
            .unwrap_or(span.end);
        Ok(self.call_tup(head, items, span.start, end))
    }

    fn call_tup(&mut self, head: NodeId, items: Vec<NodeId>, start: u32, end: u32) -> NodeId {
        let mut all = Vec::with_capacity(items.len() + 1);
        all.push(head);
        all.extend(items);
        self.arena.add(Node::Tup(all), (start, end))
    }

    fn parse_paren_items(&mut self) -> Result<(Vec<NodeId>, u32), ReadError> {
        self.next()?;
        self.parse_paren_body()
    }

    fn parse_parens(&mut self, span: Token) -> Result<NodeId, ReadError> {
        let (items, end) = self.parse_paren_body()?;
        Ok(self.arena.add(Node::Tup(items), (span.start, end)))
    }

    fn parse_paren_body(&mut self) -> Result<(Vec<NodeId>, u32), ReadError> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Some((Tok::RParen, span)) => {
                    let end = span.end;
                    self.pos += 1;
                    return Ok((items, end));
                }
                Some(_) => {
                    self.allow_title_flags = items.len() == 1
                        && matches!(self.arena.node(items[0]), Node::Sym(head) if head == "derive-MACRO");
                    items.push(self.parse_form()?);
                    self.allow_title_flags = false;
                }
                None => return Err(self.eof_err()),
            }
        }
    }

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

    fn parse_braces(&mut self, span: Token) -> Result<NodeId, ReadError> {
        let start = span.start;

        let allow_titles = std::mem::take(&mut self.allow_title_flags);
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
                        return Ok(self.arena.add(Node::Rec(fields), (start, close.end)));
                    }
                    (Tok::Ident(name), _) => {
                        match self.next()? {
                            (Tok::Colon, _) => {}
                            (_, s) => {
                                return self.err("expected `:` after record field name", s.start);
                            }
                        }
                        let value = self.parse_form()?;
                        fields.push((name, value));
                    }

                    (Tok::Title(name), _) => {
                        match self.next()? {
                            (Tok::Colon, _) => {}
                            (_, s) => {
                                return self.err("expected `:` after record field name", s.start);
                            }
                        }
                        let value = self.parse_form()?;
                        fields.push((title_flag_name(&name), value));
                    }
                    (_, s) => return self.err("expected a record field name", s.start),
                }
            }
        } else {
            let mut names = Vec::new();
            loop {
                match self.next()? {
                    (Tok::RBrace, close) => {
                        return Ok(self.arena.add(Node::Flg(names), (start, close.end)));
                    }
                    (Tok::Ident(name), _) => names.push(name),

                    (Tok::Title(name), _) if allow_titles => names.push(title_flag_name(&name)),
                    (_, s) => return self.err("expected a flag name", s.start),
                }
            }
        }
    }

    fn register_if_def_macro(&mut self, id: NodeId) {
        let Node::Tup(items) = self.arena.node(id) else {
            return;
        };
        if items.len() != 4 {
            return;
        }
        let Node::Sym(h) = self.arena.node(items[0]) else {
            return;
        };
        if h != "defmacro-MACRO" {
            return;
        }
        let Node::Sym(name) = self.arena.node(items[1]) else {
            return;
        };
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

        t.register("thing-MACRO".into(), 1);
        assert_eq!(t.arity("thing-MACRO"), None);
        assert!(t.is_ambiguous("thing-MACRO").is_some());

        assert_eq!(t.arity_qualified("dsl", "thing-MACRO"), Some(1));
    }

    #[test]
    fn same_package_two_aliases_via_repeated_register_is_collision() {
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

        assert_eq!(t.arity("if-MACRO"), Some(3));
        assert!(t.is_ambiguous("if-MACRO").is_none());
    }
}

#[cfg(test)]
mod title_flags_scope_tests {
    use super::*;

    #[test]
    fn derive_flags_literal_accepts_titlecase() {
        let (arena, roots) =
            read_file("Derive {Eq Ord Show} point").expect("Derive flags literal should read");
        assert_eq!(roots.len(), 1);

        let Node::Tup(items) = arena.node(roots[0]) else {
            panic!("expected a Tup for the Derive form");
        };
        assert!(matches!(arena.node(items[0]), Node::Sym(s) if s == "derive-MACRO"));
        let Node::Flg(names) = arena.node(items[1]) else {
            panic!("expected a flags literal for Derive's first argument");
        };
        assert_eq!(
            names,
            &["Eq".to_string(), "Ord".to_string(), "Show".to_string()]
        );
    }

    #[test]
    fn non_derive_flags_literal_rejects_titlecase() {
        let err = read_file("{Foo bar}").expect_err("a TitleCase flag name must be rejected");
        assert!(
            err.msg.contains("expected a flag name"),
            "unexpected error message: {}",
            err.msg
        );
    }

    #[test]
    fn defresource_reads_members() {
        let src = "DefResource counter {
  New: Fn {start: u32} cell-new(start)
  next: Fn {self: counter} cell-get(self)
  sum: Static Fn {values: list(u32)} counter(0)
  Drop: Fn {self: counter} unit()
}";
        let (arena, roots) = read_file(src).expect("DefResource should read");
        assert_eq!(roots.len(), 1);
        let Node::Tup(items) = arena.node(roots[0]) else {
            panic!("expected a Tup for the DefResource form");
        };
        assert!(matches!(arena.node(items[0]), Node::Sym(s) if s == "defresource-MACRO"));
        assert!(matches!(arena.node(items[1]), Node::Sym(s) if s == "counter"));
        let Node::Rec(fields) = arena.node(items[2]) else {
            panic!("expected a record of members");
        };
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["New", "next", "sum", "Drop"]);

        let sum = fields.iter().find(|(k, _)| k == "sum").unwrap().1;
        let Node::Tup(st) = arena.node(sum) else {
            panic!("expected the static marker tuple");
        };
        assert!(matches!(arena.node(st[0]), Node::Sym(s) if s == "static-MACRO"));
        let Node::Tup(fnf) = arena.node(st[1]) else {
            panic!("expected the wrapped Fn form");
        };
        assert!(matches!(arena.node(fnf[0]), Node::Sym(s) if s == "fn-MACRO"));
    }
}
