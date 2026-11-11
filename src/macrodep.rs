use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::form::{Arena, Node, NodeId};
use crate::lexer::ReadError;
use crate::macros::MacroComponent;
use crate::reader::{FormHook, MacroTable};
use crate::wit::ImportInfo;

#[derive(Default)]
pub struct MacroResolver {
    root: PathBuf,
    cache: HashMap<String, MacroComponent>,
}

impl MacroResolver {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        MacroResolver {
            root: root.into(),
            cache: HashMap::new(),
        }
    }

    pub fn resolve(&mut self, import: &ImportInfo) -> Result<&mut MacroComponent, String> {
        debug_assert!(
            import.macros,
            "resolve() is only for `macros: true` imports"
        );
        let key = import.package.clone();
        if !self.cache.contains_key(&key) {
            let path = self.locate(import)?;
            let comp = MacroComponent::from_file(&path).map_err(|e| {
                format!(
                    "import `{}` (macros): {} ({})",
                    import.path,
                    e,
                    path.display()
                )
            })?;
            self.cache.insert(key.clone(), comp);
        }
        Ok(self.cache.get_mut(&key).expect("just inserted"))
    }

    fn locate(&self, import: &ImportInfo) -> Result<PathBuf, String> {
        let mut tried = Vec::new();

        if let Some(from) = &import.from {
            let p = self.resolve_relative(from);
            if p.is_file() {
                return Ok(p);
            }
            tried.push(p.display().to_string());
        }

        let conventional = self
            .root
            .join("wit")
            .join("macros")
            .join(format!("{}.wasm", import.package.replace([':', '/'], "-")));
        if conventional.is_file() {
            return Ok(conventional);
        }
        tried.push(conventional.display().to_string());

        Err(format!(
            "import `{}` is `macros: true` but no macro component was found. \
             Build the macro library to a component and either set `from: \
             \"<path>.wasm\"` on the import or place it at \
             `wit/macros/{}.wasm`. Searched: {}",
            import.path,
            import.package.replace([':', '/'], "-"),
            tried.join(", ")
        ))
    }

    fn resolve_relative(&self, p: &str) -> PathBuf {
        let path = Path::new(p);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        }
    }

    fn register_form(
        &mut self,
        arena: &Arena,
        id: NodeId,
        macros: &mut MacroTable,
    ) -> Result<(), ReadError> {
        let Some(import) = parse_macro_import(arena, id) else {
            return Ok(());
        };
        let span = arena.span(id);
        let comp = self.resolve(&import).map_err(|msg| read_err(msg, span.0))?;
        let manifest = comp.manifest().map_err(|e| {
            read_err(
                format!("import `{}` (macros): manifest() failed: {e}", import.path),
                span.0,
            )
        })?;

        for (name, arity) in manifest {
            macros.register_foreign(&import.alias, format!("{name}-MACRO"), arity as usize);
        }
        Ok(())
    }
}

pub struct FileExpander {
    resolver: MacroResolver,

    imports: Vec<ImportInfo>,

    owners: HashMap<String, Option<usize>>,

    alias_owners: HashMap<(String, String), Option<usize>>,

    local_names: std::collections::HashSet<String>,

    local_bytes: Option<Result<Vec<u8>, String>>,
    local: Option<MacroComponent>,
}

impl FileExpander {
    pub fn for_file(
        root: impl Into<PathBuf>,
        arena: &Arena,
        roots: &[NodeId],
    ) -> Option<FileExpander> {
        let imports: Vec<ImportInfo> = roots
            .iter()
            .filter_map(|&id| parse_macro_import(arena, id))
            .collect();
        let local_names: std::collections::HashSet<String> = roots
            .iter()
            .filter_map(|&id| local_macro_name(arena, id))
            .collect();
        if imports.is_empty() && local_names.is_empty() {
            return None;
        }

        let local_bytes = if local_names.is_empty() {
            None
        } else {
            Some(crate::emit::emit_macro_component(arena, roots))
        };
        Some(FileExpander {
            resolver: MacroResolver::new(root),
            imports,
            owners: HashMap::new(),
            alias_owners: HashMap::new(),
            local_names,
            local_bytes,
            local: None,
        })
    }

    fn local_component(&mut self) -> Result<&mut MacroComponent, String> {
        if self.local.is_none() {
            let bytes = match &self.local_bytes {
                Some(Ok(b)) => b,
                Some(Err(e)) => return Err(e.clone()),
                None => return Err("internal: no local macros to compile".to_string()),
            };
            self.local = Some(MacroComponent::from_bytes(bytes)?);
        }
        Ok(self.local.as_mut().expect("just built"))
    }

    fn owner_of(&mut self, name: &str) -> Result<Option<usize>, String> {
        if let Some(found) = self.owners.get(name) {
            return Ok(*found);
        }
        let mut found = None;
        for i in 0..self.imports.len() {
            let import = &self.imports[i];
            let comp = self.resolver.resolve(import)?;
            let manifest = comp.manifest()?;
            if manifest.iter().any(|(n, _)| n == name) {
                found = Some(i);
                break;
            }
        }
        self.owners.insert(name.to_string(), found);
        Ok(found)
    }

    fn owner_for_alias(&mut self, alias: &str, name: &str) -> Result<Option<usize>, String> {
        let key = (alias.to_string(), name.to_string());
        if let Some(found) = self.alias_owners.get(&key) {
            return Ok(*found);
        }
        let mut found = None;
        for i in 0..self.imports.len() {
            if self.imports[i].alias != alias {
                continue;
            }
            let import = &self.imports[i];
            let comp = self.resolver.resolve(import)?;
            let manifest = comp.manifest()?;
            if manifest.iter().any(|(n, _)| n == name) {
                found = Some(i);
                break;
            }
        }
        self.alias_owners.insert(key, found);
        Ok(found)
    }
}

fn local_macro_name(arena: &Arena, id: NodeId) -> Option<String> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    if items.len() != 4 {
        return None;
    }
    let Node::Sym(head) = arena.node(items[0]) else {
        return None;
    };
    if head != "defmacro-MACRO" {
        return None;
    }
    match arena.node(items[1]) {
        Node::Sym(name) => Some(name.clone()),
        _ => None,
    }
}

impl crate::expand::ForeignExpander for FileExpander {
    fn expand_call(
        &mut self,
        alias: Option<&str>,
        name: &str,
        arena: &Arena,
        call_id: NodeId,
    ) -> Option<Result<(Arena, NodeId), String>> {
        if alias.is_none() && self.local_names.contains(name) {
            let args = crate::meta::arena_to_tree(arena, call_id);
            let comp = match self.local_component() {
                Ok(c) => c,
                Err(e) => return Some(Err(e)),
            };
            return Some(
                comp.expand(name, &args)
                    .map(|tree| crate::meta::tree_to_arena(&tree)),
            );
        }

        let qualified = if alias.is_none() {
            self.imports
                .iter()
                .filter_map(|import| {
                    name.strip_prefix(&format!("{}-", import.alias.to_ascii_lowercase()))
                        .map(|suffix| (import.alias.clone(), suffix.to_owned()))
                })
                .max_by_key(|(alias, _)| alias.len())
        } else {
            None
        };
        let alias = alias.or_else(|| qualified.as_ref().map(|(alias, _)| alias.as_str()));
        let name = qualified
            .as_ref()
            .map(|(_, name)| name.as_str())
            .unwrap_or(name);
        let lookup = match alias {
            Some(a) => self.owner_for_alias(a, name),
            None => self.owner_of(name),
        };
        let owner = match lookup {
            Ok(Some(i)) => i,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };

        let args = crate::meta::arena_to_tree(arena, call_id);
        let import = &self.imports[owner];
        let comp = match self.resolver.resolve(import) {
            Ok(c) => c,
            Err(e) => return Some(Err(e)),
        };
        Some(
            comp.expand(name, &args)
                .map(|tree| crate::meta::tree_to_arena(&tree)),
        )
    }
}

pub fn read_file_with_macros(
    src: &str,
    root: impl Into<PathBuf>,
) -> Result<(Arena, Vec<NodeId>), ReadError> {
    let mut resolver = MacroResolver::new(root);
    let mut macros = MacroTable::core();
    let mut hook = register_macro_imports(&mut resolver);
    crate::reader::read_with_hook(src, &mut macros, Some(hook.as_mut()))
}

pub fn register_macro_imports<'a>(resolver: &'a mut MacroResolver) -> Box<FormHook<'a>> {
    Box::new(move |arena, id, macros| resolver.register_form(arena, id, macros))
}

fn parse_macro_import(arena: &Arena, id: NodeId) -> Option<ImportInfo> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    let head = *items.first()?;
    let Node::Sym(head_name) = arena.node(head) else {
        return None;
    };
    if head_name != "import-MACRO" {
        return None;
    }
    let Node::Rec(fields) = arena.node(*items.get(1)?) else {
        return None;
    };

    let mut pkg = None;
    let mut alias = None;
    let (macros, runtime) = crate::wit::import_roles(arena, fields).ok()?;
    let mut from = None;
    for (k, v) in fields {
        match (k.as_str(), arena.node(*v)) {
            ("pkg", Node::Str(s)) => pkg = Some(s.clone()),
            ("as", Node::Sym(s)) => alias = Some(s.clone()),
            ("from", Node::Str(s)) => from = Some(s.clone()),
            _ => {}
        }
    }
    if !macros {
        return None;
    }
    let pkg_str = pkg?;
    let path = pkg_str.split('@').next().unwrap_or(&pkg_str).to_string();
    let package = path.split('/').next().unwrap_or(&path).to_string();
    let alias = alias.unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path).to_string());
    Some(ImportInfo {
        path,
        version: pkg_str
            .split_once('@')
            .map(|(_, version)| version.to_owned()),
        package,
        alias,
        macros,
        runtime,
        from,
    })
}

fn read_err(msg: impl Into<String>, at: u32) -> ReadError {
    ReadError {
        msg: msg.into(),
        at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve_err(r: &mut MacroResolver, import: &ImportInfo) -> String {
        match r.resolve(import) {
            Ok(_) => panic!("expected resolve to fail, but it succeeded"),
            Err(e) => e,
        }
    }

    fn fixture_path() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/macros.wasm"
        ))
    }

    fn macros_import(from: Option<&str>) -> ImportInfo {
        ImportInfo {
            version: None,
            path: "acme:html/dsl".to_string(),
            package: "acme:html".to_string(),
            alias: "dsl".to_string(),
            macros: true,
            runtime: false,
            from: from.map(|s| s.to_string()),
        }
    }

    #[test]
    fn resolves_macro_component_from_explicit_path() {
        let mut r = MacroResolver::new(".");
        let import = macros_import(Some(fixture_path().to_str().unwrap()));
        let comp = r.resolve(&import).expect("fixture resolves");
        let manifest = comp.manifest().expect("manifest call");

        assert_eq!(manifest.len(), 3);
        assert!(manifest.iter().any(|(n, a)| n == "identity" && *a == 1));
    }

    #[test]
    fn caches_one_instance_per_package() {
        let mut r = MacroResolver::new(".");
        let import = macros_import(Some(fixture_path().to_str().unwrap()));
        r.resolve(&import).expect("first resolve");
        assert_eq!(r.cache.len(), 1);

        let mut second = macros_import(Some(fixture_path().to_str().unwrap()));
        second.alias = "html".to_string();
        r.resolve(&second).expect("second resolve");
        assert_eq!(r.cache.len(), 1, "same package must not re-instantiate");
    }

    #[test]
    fn resolves_from_conventional_location() {
        let tmp = std::env::temp_dir().join(format!(
            "wavelet-macrodep-{}-{}",
            std::process::id(),
            line!()
        ));
        let macros_dir = tmp.join("wit").join("macros");
        std::fs::create_dir_all(&macros_dir).unwrap();
        let dest = macros_dir.join("acme-html.wasm");
        std::fs::copy(fixture_path(), &dest).unwrap();

        let mut r = MacroResolver::new(&tmp);
        let import = macros_import(None);
        let comp = r.resolve(&import).expect("conventional path resolves");
        comp.manifest().expect("manifest call");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn missing_binary_is_actionable() {
        let mut r = MacroResolver::new("/no/such/project");
        let import = macros_import(Some("does-not-exist.wasm"));
        let err = resolve_err(&mut r, &import);
        assert!(err.contains("macros: true"), "unexpected error: {err}");
        assert!(
            err.contains("does-not-exist.wasm"),
            "should name the from path: {err}"
        );
        assert!(
            err.contains("wit/macros/acme-html.wasm"),
            "should name the convention: {err}"
        );
    }

    #[test]
    fn non_macro_component_is_rejected() {
        let add = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/add.wasm"
        ));
        let mut r = MacroResolver::new(".");
        let import = macros_import(Some(add.to_str().unwrap()));
        let err = resolve_err(&mut r, &import);

        assert!(
            err.contains("wavelet:meta/macros") || err.contains("does not export"),
            "unexpected error: {err}"
        );
    }

    fn src_importing_fixture(tail: &str) -> String {
        format!(
            "Package \"demo:app@0.1.0\"\n\
             Import {{pkg: \"acme:html/dsl\" macros: true from: \"{}\"}}\n\
             {tail}\n",
            fixture_path().to_str().unwrap()
        )
    }

    fn read(src: &str) -> Result<(Arena, Vec<NodeId>), ReadError> {
        read_file_with_macros(src, env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn foreign_macro_is_unknown_without_registration() {
        let src = "Package \"demo:app@0.1.0\"\nUnless false \"ran\"\n";
        let err = crate::reader::read_file(src).expect_err("Unless is unknown");
        assert!(
            err.msg.contains("unknown macro") && err.msg.contains("unless-MACRO"),
            "unexpected error: {}",
            err.msg
        );
    }

    #[test]
    fn foreign_arity_two_macro_reads_paren_free() {
        let src = src_importing_fixture("Unless false \"ran\"");
        let (arena, roots) = read(&src).expect("reads with foreign arity");

        let last = *roots.last().unwrap();

        assert_eq!(
            crate::printer::print(&arena, last),
            r#"(unless-MACRO, false, "ran")"#
        );
    }

    #[test]
    fn foreign_arity_zero_macro_consumes_nothing() {
        let src = src_importing_fixture("Boom\n42");
        let (arena, roots) = read(&src).expect("reads zero-arity foreign macro");

        assert_eq!(roots.len(), 4, "Boom must not consume the following form");
        assert_eq!(crate::printer::print(&arena, roots[2]), "(boom-MACRO)");
        assert_eq!(crate::printer::print(&arena, roots[3]), "42");
    }

    #[test]
    fn foreign_arity_one_macro_reads_paren_free() {
        let src = src_importing_fixture("Identity add(1 2)");
        let (arena, roots) = read(&src).expect("reads one-arity foreign macro");
        let last = *roots.last().unwrap();
        assert_eq!(
            crate::printer::print(&arena, last),
            "(identity-MACRO, (add, 1, 2))"
        );
    }

    #[test]
    fn foreign_macro_explicit_payload_reads() {
        let src = src_importing_fixture(r#"Unless(false "ran")"#);
        let (arena, roots) = read(&src).expect("reads explicit-payload foreign macro");
        let last = *roots.last().unwrap();
        assert_eq!(
            crate::printer::print(&arena, last),
            r#"(unless-MACRO, false, "ran")"#
        );
    }

    #[test]
    fn unresolvable_macro_import_errors_at_read_time() {
        let src = "Package \"demo:app@0.1.0\"\n\
                   Import {pkg: \"acme:html/dsl\" macros: true from: \"nope.wasm\"}\n";
        let err = read_file_with_macros(src, "/no/such/project").expect_err("resolve fails");
        assert!(
            err.msg.contains("macros: true"),
            "unexpected error: {}",
            err.msg
        );
        assert!(
            err.msg.contains("nope.wasm"),
            "should name the from path: {}",
            err.msg
        );
    }

    #[test]
    fn non_macro_import_registers_nothing() {
        let src = "Package \"demo:app@0.1.0\"\n\
                   Import \"acme:html/dsl\"\n\
                   Unless false \"ran\"\n";
        let err = read_file_with_macros(src, env!("CARGO_MANIFEST_DIR"))
            .expect_err("non-macro import does not register Unless");
        assert!(
            err.msg.contains("unknown macro"),
            "unexpected error: {}",
            err.msg
        );
    }

    fn src_two_aliases(tail: &str) -> String {
        let p = fixture_path();
        let p = p.to_str().unwrap();
        format!(
            "Package \"demo:app@0.1.0\"\n\
             Import {{pkg: \"acme:html/dsl\" macros: true from: \"{p}\"}}\n\
             Import {{pkg: \"other:html/web\" macros: true from: \"{p}\"}}\n\
             {tail}\n"
        )
    }

    #[test]
    fn colliding_bare_use_is_ambiguous_error() {
        let src = src_two_aliases("Unless false \"ran\"");
        let err = read(&src).expect_err("bare Unless is ambiguous across two imports");
        assert!(
            err.msg.contains("ambiguous"),
            "unexpected error: {}",
            err.msg
        );
        assert!(
            err.msg.contains("unless"),
            "should name the macro: {}",
            err.msg
        );

        assert!(
            err.msg.contains("dsl/unless") && err.msg.contains("web/unless"),
            "should suggest qualified spellings: {}",
            err.msg
        );
    }

    #[test]
    fn qualified_use_resolves_despite_collision() {
        let src = src_two_aliases("dsl/Unless false \"a\"\nweb/Unless true \"b\"");
        let (arena, roots) = read(&src).expect("qualified uses resolve despite collision");

        assert_eq!(
            crate::printer::print(&arena, roots[3]),
            r#"(dsl-unless-MACRO, false, "a")"#
        );
        assert_eq!(
            crate::printer::print(&arena, roots[4]),
            r#"(web-unless-MACRO, true, "b")"#
        );
    }

    #[test]
    fn unknown_qualified_macro_errors() {
        let src = src_two_aliases("dsl/Nope 1");
        let err = read(&src).expect_err("dsl/nope is unknown");
        assert!(
            err.msg.contains("unknown macro") && err.msg.contains("dsl-nope"),
            "unexpected error: {}",
            err.msg
        );
    }

    #[test]
    fn explicit_as_alias_resolves_qualified() {
        let src = format!(
            "Package \"demo:app@0.1.0\"\n\
             Import {{pkg: \"acme:html/elements\" as: dsl macros: true from: \"{}\"}}\n\
             dsl/Unless false \"ran\"\n",
            fixture_path().to_str().unwrap()
        );
        let (arena, roots) = read(&src).expect("aliased import resolves qualified head");
        let last = *roots.last().unwrap();
        assert_eq!(
            crate::printer::print(&arena, last),
            r#"(dsl-unless-MACRO, false, "ran")"#
        );

        let bad = format!(
            "Package \"demo:app@0.1.0\"\n\
             Import {{pkg: \"acme:html/elements\" as: dsl macros: true from: \"{}\"}}\n\
             elements/Unless false \"ran\"\n",
            fixture_path().to_str().unwrap()
        );
        let err = read(&bad).expect_err("elements is not the alias");
        assert!(err.msg.contains("unknown macro"), "unexpected: {}", err.msg);
    }

    #[test]
    fn qualified_call_expands_through_aliased_component() {
        let src = src_two_aliases("web/Identity 99");
        let (arena, roots) = read(&src).expect("reads qualified call");
        let mut fx = FileExpander::for_file(env!("CARGO_MANIFEST_DIR"), &arena, &roots)
            .expect("file imports macros");
        let (out, new_roots) =
            crate::expand::expand_file(arena, &roots, Some(&mut fx)).expect("expands");

        let last = *new_roots.last().unwrap();
        assert_eq!(crate::printer::print(&out, last), "99");
    }

    #[test]
    fn single_import_bare_use_unchanged() {
        let src = src_importing_fixture("Unless false \"ran\"");
        let (arena, roots) = read(&src).expect("single-import bare use still reads");
        let last = *roots.last().unwrap();
        assert_eq!(
            crate::printer::print(&arena, last),
            r#"(unless-MACRO, false, "ran")"#
        );
    }
}
