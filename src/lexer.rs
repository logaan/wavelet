use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Colon,
    /// `.` joining a receiver to a chained call (`recv.name(...)`, §2.5)
    Dot,
    Bool(bool),
    Int(i64),
    Dec(f64),
    Char(char),
    Str(String),
    /// kebab-case identifier (WIT label); `%` escape already stripped
    Ident(String),
    /// TitleCase macro head, already lowercased with `-MACRO` suffix
    Title(String),
    /// `alias/name`; bool = name part was TitleCase (already suffixed)
    QIdent(String, String, bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Token {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug)]
pub struct ReadError {
    pub msg: String,
    pub at: u32,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "read error at byte {}: {}", self.at, self.msg)
    }
}

impl std::error::Error for ReadError {}

fn err<T>(msg: impl Into<String>, at: u32) -> Result<T, ReadError> {
    Err(ReadError { msg: msg.into(), at })
}

pub fn lex(src: &str) -> Result<Vec<(Tok, Token)>, ReadError> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        match c {
            ' ' | '\t' | '\r' | '\n' | ',' => i += 1,
            '/' if b.get(i + 1) == Some(&b'/') => {
                // `//` to end of line is a comment and leaves no token (this
                // includes `///`, which is no longer special).
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            '(' | ')' | '[' | ']' | '{' | '}' | ':' | '.' => {
                let tok = match c {
                    '(' => Tok::LParen,
                    ')' => Tok::RParen,
                    '[' => Tok::LBracket,
                    ']' => Tok::RBracket,
                    '{' => Tok::LBrace,
                    '}' => Tok::RBrace,
                    ':' => Tok::Colon,
                    _ => Tok::Dot,
                };
                out.push((tok, span(i, i + 1)));
                i += 1;
            }
            '"' => {
                let start = i;
                let (s, next) = lex_string(src, i)?;
                out.push((Tok::Str(s), span(start, next)));
                i = next;
            }
            '\'' => {
                let start = i;
                let (ch, next) = lex_char(src, i)?;
                out.push((Tok::Char(ch), span(start, next)));
                i = next;
            }
            '-' | '0'..='9' => {
                let start = i;
                // Whole-word match only, like the positive `inf` path in
                // `lex_name`: `-inf` must not be a prefix of a longer token
                // (`-info`, `-infinity`).
                if c == '-'
                    && src[i + 1..].starts_with("inf")
                    && b.get(i + 4).is_none_or(|&ch| !is_name_char(ch))
                {
                    out.push((Tok::Dec(f64::NEG_INFINITY), span(start, start + 4)));
                    i += 4;
                } else {
                    let (tok, next) = lex_number(src, i)?;
                    out.push((tok, span(start, next)));
                    i = next;
                }
            }
            '%' | 'a'..='z' | 'A'..='Z' => {
                let start = i;
                let (tok, next) = lex_name(src, i)?;
                i = next;
                // qualified name: name "/" name with no whitespace
                if b.get(i) == Some(&b'/') && b.get(i + 1) != Some(&b'/') {
                    let (tok2, next2) = lex_name(src, i + 1)?;
                    let alias = match tok {
                        Tok::Ident(s) => s,
                        _ => return err("alias part of a qualified name must be kebab-case", start as u32),
                    };
                    let q = match tok2 {
                        Tok::Ident(s) => Tok::QIdent(alias, s, false),
                        Tok::Title(s) => Tok::QIdent(alias, s, true),
                        _ => return err("bad qualified name", start as u32),
                    };
                    out.push((q, span(start, next2)));
                    i = next2;
                } else {
                    out.push((tok, span(start, i)));
                }
            }
            _ => return err(format!("unexpected character {c:?}"), i as u32),
        }
    }
    Ok(out)
}

fn span(start: usize, end: usize) -> Token {
    Token { start: start as u32, end: end as u32 }
}

fn is_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-'
}

fn lex_name(src: &str, start: usize) -> Result<(Tok, usize), ReadError> {
    let b = src.as_bytes();
    let mut i = start;
    let escaped = b[i] == b'%';
    if escaped {
        i += 1;
    }
    let word_start = i;
    while i < b.len() && is_name_char(b[i]) {
        i += 1;
    }
    let text = &src[word_start..i];
    if text.is_empty() {
        return err("expected identifier", start as u32);
    }
    if !escaped {
        match text {
            "true" => return Ok((Tok::Bool(true), i)),
            "false" => return Ok((Tok::Bool(false), i)),
            "inf" => return Ok((Tok::Dec(f64::INFINITY), i)),
            "nan" => return Ok((Tok::Dec(f64::NAN), i)),
            _ => {}
        }
    }
    if is_title(text) {
        if escaped {
            return err("`%` escape applies to kebab-case identifiers only", start as u32);
        }
        Ok((Tok::Title(title_to_macro_name(text)), i))
    } else {
        validate_kebab(text, start as u32)?;
        Ok((Tok::Ident(text.to_string()), i))
    }
}

/// TitleCase macro head: the **first** hyphen-separated word is Title-case —
/// it starts with a capital and contains at least one lowercase letter
/// (`If`, `DefMacro`, `Try-let`). A Title-case leading word can't be a WIT
/// label word (those are all-lower or all-UPPER), so this never collides with
/// an ordinary identifier such as `parse-JSON` or `HTTP-get`.
fn is_title(text: &str) -> bool {
    let first = text.split('-').next().unwrap_or("");
    first.starts_with(|c: char| c.is_ascii_uppercase())
        && first.contains(|c: char| c.is_ascii_lowercase())
}

/// `TryLet` -> `trylet-MACRO`, `DefMacro` -> `defmacro-MACRO`,
/// `Try-let` -> `try-let-MACRO`.
///
/// The token is lowercased wholesale; there is no internal capitalisation
/// spreading (an interior capital does *not* introduce a hyphen). Existing
/// hyphens are preserved, so a hyphenated head like `Try-let` maps onto the
/// kebab-named macro `try-let`; a single TitleCase word like `TryLet` maps
/// onto `trylet`.
pub fn title_to_macro_name(text: &str) -> String {
    let mut out = text.to_ascii_lowercase();
    out.push_str("-MACRO");
    out
}

/// Each hyphen-separated word must be all-lowercase or all-UPPERCASE (WIT labels).
fn validate_kebab(text: &str, at: u32) -> Result<(), ReadError> {
    for word in text.split('-') {
        let ok = !word.is_empty()
            && word.starts_with(|c: char| c.is_ascii_alphabetic())
            && (word.chars().all(|c| !c.is_ascii_uppercase())
                || word.chars().all(|c| !c.is_ascii_lowercase()));
        if !ok {
            return err(format!("invalid identifier {text:?}"), at);
        }
    }
    Ok(())
}

fn lex_number(src: &str, start: usize) -> Result<(Tok, usize), ReadError> {
    let b = src.as_bytes();
    let mut i = start;
    if b[i] == b'-' {
        i += 1;
    }
    let mut is_dec = false;
    while i < b.len() {
        match b[i] {
            b'0'..=b'9' => i += 1,
            // a `.` is a decimal point only when a digit follows; otherwise it
            // ends the number and is lexed as a chain `.` (`1.increment()`, §2.5)
            b'.' if b.get(i + 1).is_some_and(|d| d.is_ascii_digit()) => {
                is_dec = true;
                i += 1;
            }
            b'e' | b'E' => {
                is_dec = true;
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    let text = &src[start..i];
    let tok = if is_dec {
        match text.parse::<f64>() {
            Ok(f) => Tok::Dec(f),
            Err(_) => return err(format!("invalid float literal {text:?}"), start as u32),
        }
    } else {
        match text.parse::<i64>() {
            Ok(n) => Tok::Int(n),
            Err(_) => return err(format!("invalid integer literal {text:?}"), start as u32),
        }
    };
    Ok((tok, i))
}

fn lex_string(src: &str, start: usize) -> Result<(String, usize), ReadError> {
    let mut out = String::new();
    let mut it = src[start + 1..].char_indices();
    while let Some((off, c)) = it.next() {
        let pos = start + 1 + off;
        match c {
            '"' => return Ok((out, pos + 1)),
            '\\' => out.push(read_escape(pos, &mut it)?),
            _ => out.push(c),
        }
    }
    err("unterminated string", start as u32)
}

fn lex_char(src: &str, start: usize) -> Result<(char, usize), ReadError> {
    let mut it = src[start + 1..].char_indices();
    let (off, c) = match it.next() {
        Some(x) => x,
        None => return err("unterminated char literal", start as u32),
    };
    let pos = start + 1 + off;
    let ch = match c {
        '\\' => read_escape(pos, &mut it)?,
        '\'' => return err("empty char literal", start as u32),
        _ => c,
    };
    match it.next() {
        Some((off2, '\'')) => Ok((ch, start + 1 + off2 + 1)),
        _ => err("unterminated char literal", start as u32),
    }
}

fn read_escape(
    backslash_pos: usize,
    it: &mut std::str::CharIndices<'_>,
) -> Result<char, ReadError> {
    let at = backslash_pos as u32;
    let (_, c) = match it.next() {
        Some(x) => x,
        None => return err("dangling escape", at),
    };
    let ch = match c {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        '\\' => '\\',
        '"' => '"',
        '\'' => '\'',
        'u' => {
            match it.next() {
                Some((_, '{')) => {}
                _ => return err("expected `{` after \\u", at),
            }
            let mut hex = String::new();
            loop {
                match it.next() {
                    Some((_, '}')) => break,
                    Some((_, h)) if h.is_ascii_hexdigit() => hex.push(h),
                    _ => return err("bad \\u{...} escape", at),
                }
            }
            let code = u32::from_str_radix(&hex, 16)
                .ok()
                .and_then(char::from_u32);
            match code {
                Some(ch) => ch,
                None => return err("invalid unicode scalar in \\u{...}", at),
            }
        }
        other => return err(format!("unknown escape \\{other}"), at),
    };
    Ok(ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src).expect("lex").into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn neg_inf_is_a_whole_word() {
        // bare `-inf` is the negative-infinity literal, including at a boundary
        assert_eq!(toks("-inf"), vec![Tok::Dec(f64::NEG_INFINITY)]);
        assert_eq!(toks("(-inf)"), vec![Tok::LParen, Tok::Dec(f64::NEG_INFINITY), Tok::RParen]);
        // it must not be split out of a longer token (the old prefix match gave
        // `[-inf, o]`); since identifiers can't begin with `-`, the leftover
        // `-` is now a lex error rather than a bogus split.
        assert!(lex("-info").is_err());
        assert!(lex("-infinity").is_err());
        // positive `inf` is already whole-word; the symmetry now holds
        assert_eq!(toks("inf"), vec![Tok::Dec(f64::INFINITY)]);
        assert_eq!(toks("info"), vec![Tok::Ident("info".into())]);
    }
}
