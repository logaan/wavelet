use std::collections::HashMap;

use wavelet::host::{HostComponent, Val};
use wavelet::{emit, expand, read_file, wit, witdep};

#[test]
fn external_resource_only_interfaces_export_and_execute() {
    for version in ["", "@1.0.0"] {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("counter.wit"),
            format!("package acme:counter{version}; interface api {{ resource counter {{ constructor(start: u32); value: func() -> u32; }} }}"),
        ).unwrap();
        let dependency = witdep::resolve_dep(directory.path(), "acme:counter")
            .unwrap()
            .unwrap();
        let source = format!(
            r#"
Package "app:implementation"
DefResource counter {{
    New: Fn {{start: u32}} start
    value: Fn {{self: counter}} The u32 self
}}
Export {{iface: "acme:counter/api{version}" name: counter}}
"#
        );
        let (arena, roots) = read_file(&source).unwrap();
        let (arena, roots) = expand::expand_file(arena, &roots, None).unwrap();
        let info = wit::collect(&arena, &roots).unwrap();
        let dependencies = HashMap::from([("acme:counter".to_owned(), dependency)]);
        let bytes = emit::emit_component(&arena, &roots, &info, &dependencies).unwrap();
        let mut host = HostComponent::from_bytes(&bytes).unwrap();
        let interface = format!("acme:counter/api{version}");
        let mut results = host
            .call_instance(&interface, "[constructor]counter", &[Val::U32(42)])
            .unwrap();
        assert_eq!(results.len(), 1);
        let handle = results.remove(0);
        assert!(matches!(handle, Val::Resource(_)));
        for _ in 0..2 {
            assert_eq!(
                host.call_instance(
                    &interface,
                    "[method]counter.value",
                    std::slice::from_ref(&handle)
                )
                .unwrap(),
                vec![Val::U32(42)]
            );
        }
        host.drop_resource(handle).unwrap();
    }
}
