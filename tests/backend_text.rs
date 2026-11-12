use wavelet::host::{HostComponent, Val};

#[test]
fn case_conversion_names_are_not_builtins() {
    for name in ["upper", "lower"] {
        let expression = format!("{name}(\"Straße\")");
        let outcome = wavelet::eval_snippet(&expression);
        assert!(!outcome.ok, "{expression} unexpectedly evaluated");
        assert!(outcome.error.contains(name), "{}", outcome.error);
        let source = format!("Package \"test:removed\" Export run Def run Fn {{}} {expression}");
        let error = wavelet::staging::Project::compile(&source).unwrap_err();
        assert!(error.to_string().contains(name), "{error}");
    }
}

#[test]
fn unicode_text_and_scalar_lengths_remain_supported() {
    let source = r#"Package "test:text"
Export echo
Def echo Fn {text: string} str-cat(text "!")
Export length
Def length Fn {text: string} len(text)
"#;
    let bytes = wavelet::staging::Project::compile(source).unwrap();
    let mut component = HostComponent::from_bytes(&bytes).unwrap();
    for input in ["", "Straße ﬃ", "ΟΣ ΟΣΑ", "😀e\u{301}", "\0東京"] {
        assert_eq!(
            component
                .call_instance("test:text/api", "echo", &[Val::String(input.into())])
                .unwrap(),
            vec![Val::String(format!("{input}!"))]
        );
        assert_eq!(
            component
                .call_instance("test:text/api", "length", &[Val::String(input.into())])
                .unwrap(),
            vec![Val::S64(input.chars().count() as i64)]
        );
    }
}
