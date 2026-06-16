use crate::form::{Arena, Node, NodeId};

/// Canonical WAVE text: strict commas, escapes normalized (§2.1).
pub fn print(arena: &Arena, id: NodeId) -> String {
    let mut out = String::new();
    write_form(arena, id, &mut out);
    out
}

fn write_form(arena: &Arena, id: NodeId, out: &mut String) {
    match arena.node(id) {
        Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Node::Int(n) => out.push_str(&n.to_string()),
        Node::Dec(f) => out.push_str(&print_dec(*f)),
        Node::Char(c) => {
            out.push('\'');
            write_escaped(*c, '\'', out);
            out.push('\'');
        }
        Node::Str(s) => {
            out.push('"');
            for c in s.chars() {
                write_escaped(c, '"', out);
            }
            out.push('"');
        }
        Node::Sym(name) => out.push_str(name),
        Node::Qsym(alias, name) => {
            out.push_str(alias);
            out.push('/');
            out.push_str(name);
        }
        Node::Tup(items) => write_seq(arena, items, '(', ')', out),
        Node::Lst(items) => write_seq(arena, items, '[', ']', out),
        Node::Rec(fields) => {
            out.push('{');
            for (i, (name, value)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(name);
                out.push_str(": ");
                write_form(arena, *value, out);
            }
            out.push('}');
        }
        Node::Flg(names) => {
            out.push('{');
            for (i, name) in names.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(name);
            }
            out.push('}');
        }
    }
}

fn write_seq(arena: &Arena, items: &[NodeId], open: char, close: char, out: &mut String) {
    out.push(open);
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_form(arena, *item, out);
    }
    out.push(close);
}

fn print_dec(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f == f64::INFINITY {
        "inf".into()
    } else if f == f64::NEG_INFINITY {
        "-inf".into()
    } else {
        format!("{f:?}")
    }
}

fn write_escaped(c: char, quote: char, out: &mut String) {
    match c {
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        '\r' => out.push_str("\\r"),
        c if c == quote => {
            out.push('\\');
            out.push(c);
        }
        c if (c as u32) < 0x20 => {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        }
        c => out.push(c),
    }
}
