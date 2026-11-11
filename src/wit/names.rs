use std::borrow::Cow;

pub(crate) fn identifier(name: &str) -> Cow<'_, str> {
    const KEYWORDS: &[&str] = &[
        "use",
        "type",
        "func",
        "u8",
        "u16",
        "u32",
        "u64",
        "s8",
        "s16",
        "s32",
        "s64",
        "f32",
        "f64",
        "float32",
        "float64",
        "char",
        "resource",
        "record",
        "flags",
        "variant",
        "enum",
        "bool",
        "string",
        "option",
        "result",
        "future",
        "stream",
        "list",
        "own",
        "borrow",
        "_",
        "as",
        "from",
        "static",
        "interface",
        "tuple",
        "world",
        "import",
        "export",
        "package",
        "with",
        "include",
        "constructor",
        "error-context",
        "async",
        "map",
    ];
    if KEYWORDS.contains(&name) {
        Cow::Owned(format!("%{name}"))
    } else {
        Cow::Borrowed(name)
    }
}

pub(crate) fn path(name: &str) -> String {
    let (base, version) = name
        .split_once('@')
        .map_or((name, None), |(base, version)| (base, Some(version)));
    let mut out = String::new();
    for part in base.split_inclusive([':', '/', '.']) {
        let end = part.trim_end_matches([':', '/', '.']).len();
        out.push_str(&identifier(&part[..end]));
        out.push_str(&part[end..]);
    }
    if let Some(version) = version {
        out.push('@');
        out.push_str(version);
    }
    out
}

pub(crate) fn type_expression(text: &str) -> String {
    let mut out = String::new();
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        if !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '%' | '.')) {
            type_token(&mut out, &text[start..index], ch == '<');
            out.push(ch);
            start = index + ch.len_utf8();
        }
    }
    type_token(&mut out, &text[start..], false);
    out
}

fn type_token(out: &mut String, token: &str, constructor: bool) {
    const PRIMITIVES: &[&str] = &[
        "bool",
        "u8",
        "u16",
        "u32",
        "u64",
        "s8",
        "s16",
        "s32",
        "s64",
        "f32",
        "f64",
        "char",
        "string",
        "result",
        "error-context",
        "_",
    ];
    if constructor || PRIMITIVES.contains(&token) {
        out.push_str(token);
    } else {
        out.push_str(&path(token));
    }
}

pub(crate) fn identifiers(names: &[String]) -> String {
    names
        .iter()
        .map(|name| identifier(name))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn parameters(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(name, ty)| format!("{}: {}", identifier(name), type_expression(ty)))
        .collect::<Vec<_>>()
        .join(", ")
}
