pub mod form;
pub mod lexer;
pub mod printer;
pub mod reader;

pub use form::{Arena, Node, NodeId};
pub use lexer::ReadError;
pub use printer::print;
pub use reader::read_file;

#[cfg(test)]
mod tests {
    use super::*;

    fn read1(src: &str) -> String {
        let (arena, roots) = read_file(src).expect(src);
        assert_eq!(roots.len(), 1, "expected one form in {src:?}");
        print(&arena, roots[0])
    }

    fn read_all(src: &str) -> Vec<String> {
        let (arena, roots) = read_file(src).expect(src);
        roots.iter().map(|&r| print(&arena, r)).collect()
    }

    #[test]
    fn atoms() {
        assert_eq!(read1("true"), "true");
        assert_eq!(read1("42"), "42");
        assert_eq!(read1("-1.5"), "-1.5");
        assert_eq!(read1("6.022e+23"), "6.022e23");
        assert_eq!(read1("-inf"), "-inf");
        assert_eq!(read1("nan"), "nan");
        assert_eq!(read1("'x'"), "'x'");
        assert_eq!(read1("'☃'"), "'☃'");
        assert_eq!(read1(r#"'\u{0}'"#), r#"'\u{0}'"#);
        assert_eq!(read1(r#""abc\t123""#), r#""abc\t123""#);
    }

    #[test]
    fn desugar_table() {
        assert_eq!(read1("foo"), "foo");
        assert_eq!(read1("f(x)"), "f(x)");
        assert_eq!(read1("f(x y)"), "f((x, y))");
        assert_eq!(read1("f[x y]"), "f([x, y])");
        assert_eq!(read1("f{a: 1 b: 2}"), "f({a: 1, b: 2})");
        assert_eq!(read1("f[]"), "f([])");
        assert_eq!(read1("f()"), "f([])");
        assert_eq!(read1("kv/get{bucket: b}"), "kv/get({bucket: b})");
        assert_eq!(read1("(a b)"), "(a, b)");
        assert_eq!(read1("(a)"), "a");
        assert_eq!(read1("[a b]"), "[a, b]");
        assert_eq!(read1("{k: v}"), "{k: v}");
        assert_eq!(read1("{read write}"), "{read, write}");
        assert_eq!(read1("{}"), "{}");
        assert_eq!(read1("If c t e"), "if-MACRO((c, t, e))");
        assert_eq!(read1("Unquote(x)"), "unquote-MACRO(x)");
    }

    #[test]
    fn commas_are_whitespace() {
        assert_eq!(read1("[1, 2, 3]"), read1("[1 2 3]"));
        assert_eq!(read1("f(x, y)"), "f((x, y))");
    }

    #[test]
    fn attachment_rule() {
        assert_eq!(
            read1(r#"delete-file{path: "foo.md" force: true}"#),
            r#"delete-file({path: "foo.md", force: true})"#
        );
        assert_eq!(
            read_all(r#"delete-file {path: "foo.md" force: true}"#),
            vec!["delete-file".to_string(), r#"{path: "foo.md", force: true}"#.to_string()]
        );
    }

    #[test]
    fn macro_arity_reading() {
        assert_eq!(read1("Quote foo"), "quote-MACRO(foo)");
        assert_eq!(
            read1(r#"If eq[foo bar] print("match") print("nope")"#),
            r#"if-MACRO((eq([foo, bar]), print("match"), print("nope")))"#
        );
        // nested macro forms need no delimiters
        assert_eq!(
            read1("Def run Fn {} If c a b"),
            "def-MACRO((run, fn-MACRO(({}, if-MACRO((c, a, b))))))"
        );
        // explicit payload overrides arity reading
        assert_eq!(read1("If(c t e)"), "if-MACRO((c, t, e))");
    }

    #[test]
    fn def_macro_registers_arity() {
        let forms = read_all(
            "DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false\n\
             And lt[x 10] gt[x 0]",
        );
        assert_eq!(forms.len(), 2);
        assert_eq!(forms[1], "and-MACRO((lt([x, 10]), gt([x, 0])))");
    }

    #[test]
    fn unknown_macro_is_an_error() {
        assert!(read_file("Bogus 1 2").is_err());
    }

    #[test]
    fn title_case_does_not_collide_with_upper_words() {
        assert_eq!(read1("parse-JSON(x)"), "parse-JSON(x)");
        // TryLet is not a core form, so it needs an explicit payload here
        assert_eq!(read1("TryLet({a: b} c)"), "try-let-MACRO(({a: b}, c))");
    }

    #[test]
    fn comments_and_newlines() {
        assert_eq!(
            read1("// leading comment\nf(x) // trailing"),
            "f(x)"
        );
    }

    #[test]
    fn match_clause_shape() {
        assert_eq!(
            read1("Match r [ (ok(text) process(text)) (err(e) handle(e)) ]"),
            "match-MACRO((r, [(ok(text), process(text)), (err(e), handle(e))]))"
        );
    }

    #[test]
    fn round_trip_is_stable() {
        for src in [
            "f((x, y))",
            "if-MACRO((c, t, e))",
            r#"delete-file({path: "foo.md", force: true})"#,
            "[1, 2, [3, (a, b)], {f: {read, write}}]",
        ] {
            assert_eq!(read1(src), src, "canonical text must read back unchanged");
        }
    }
}
