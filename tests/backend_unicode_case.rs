use wavelet::host::{HostComponent, Val};

fn casing_component() -> HostComponent {
    let source = r#"Package "test:unicode@0.1.0"
Export {name: up params: {text: string} result: string}
Def up Fn {text: string} upper(text)
Export {name: down params: {text: string} result: string}
Def down Fn {text: string} lower(text)
"#;
    let bytes = wavelet::staging::Project::compile(source).unwrap();
    HostComponent::from_bytes(&bytes).unwrap()
}

fn assert_case(component: &mut HostComponent, operation: &str, input: &str, expected: &str) {
    let values = component
        .call_instance(
            "test:unicode/api@0.1.0",
            operation,
            &[Val::String(input.into())],
        )
        .unwrap();
    let [Val::String(actual)] = values.as_slice() else {
        panic!("unexpected result: {values:?}")
    };
    assert_eq!(actual, expected, "{operation}: {input:?}");
}

#[test]
fn unicode_expansions_and_contextual_sigma_match_rust() {
    let mut component = casing_component();
    for input in [
        "",
        "aBc 123 !",
        "Straße ﬃ İ ǰ ΐ",
        "École Ελληνικά Кириллица 𐐀𐐨 😀",
        "Σ ΟΣ ΟΣΑ AΣ AΣA AΣ1 1Σ",
        "A\u{301}Σ AΣ\u{301} AΣ\u{301}A",
        "A\u{345}Σ AΣ\u{345}A \u{345}Σ",
        "A'Σ AΣ'A AΣ' A.Σ AΣ.A",
        "A\u{200d}Σ AΣ\u{200d}A",
        "\0AΣ\0AΣA\0",
    ] {
        assert_case(&mut component, "up", input, &input.to_uppercase());
        assert_case(&mut component, "down", input, &input.to_lowercase());
    }
    let ignored = "\u{301}".repeat(20_000);
    for input in [format!("A{ignored}Σ{ignored}"), format!("AΣ{ignored}A")] {
        assert_case(&mut component, "down", &input, &input.to_lowercase());
    }
}

#[test]
fn every_unicode_scalar_and_sigma_context_matches_rust() {
    let mut component = casing_component();
    for block in (0..=0x10ffff).step_by(4096) {
        let mut scalars = String::new();
        let mut contexts = String::new();
        for c in (block..=(block + 4095).min(0x10ffff)).filter_map(char::from_u32) {
            scalars.push(c);
            contexts.push_str(&format!("{c}Σ A{c}Σ AΣ{c}A "));
        }
        assert_case(&mut component, "up", &scalars, &scalars.to_uppercase());
        assert_case(&mut component, "down", &scalars, &scalars.to_lowercase());
        assert_case(&mut component, "down", &contexts, &contexts.to_lowercase());
    }
}

#[test]
fn components_without_case_conversion_omit_unicode_tables() {
    let bytes = wavelet::staging::Project::compile(
        "Package \"test:small@0.1.0\" Export {name: add-one params: {n: u32} result: u32} Def add-one Fn {n: u32} add(n 1)",
    ).unwrap();
    assert!(bytes.len() < 16_384, "component size: {}", bytes.len());
    let mut component = HostComponent::from_bytes(&bytes).unwrap();
    assert_eq!(
        component
            .call_instance("test:small/api@0.1.0", "add-one", &[Val::U32(41)])
            .unwrap(),
        vec![Val::U32(42)]
    );
}

#[test]
fn aliases_to_case_conversion_keep_unicode_tables() {
    let bytes = wavelet::staging::Project::compile(
        "Package \"test:alias@0.1.0\" Export {name: convert params: {text: string} result: string} Def convert Fn {text: string} Let {operation: Fn {value: string} upper(value)} operation(text)",
    ).unwrap();
    let mut component = HostComponent::from_bytes(&bytes).unwrap();
    assert_eq!(
        component
            .call_instance(
                "test:alias/api@0.1.0",
                "convert",
                &[Val::String("Straße".into())]
            )
            .unwrap(),
        vec![Val::String("STRASSE".into())]
    );
}
