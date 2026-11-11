use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};

use super::{MacroHost, MacroImport, MacroRequest, MacroSource};
use crate::form::{Arena, Node, NodeId};
use crate::lexer::ReadError;
use crate::reader::MacroTable;
use crate::wit::ImportInfo;

type Manifest = Vec<(String, u32)>;

pub(super) struct Catalog<'a> {
    pub pending: Vec<MacroRequest>,
    host: &'a mut dyn MacroHost,
    manifests: HashMap<String, Manifest>,
    project_libraries: HashMap<String, (String, Vec<u8>)>,
}

impl<'a> Catalog<'a> {
    pub(super) fn new(host: &'a mut dyn MacroHost) -> Self {
        Self {
            pending: Vec::new(),
            host,
            manifests: HashMap::new(),
            project_libraries: HashMap::new(),
        }
    }

    pub(super) fn index_libraries(
        &mut self,
        sources: &[super::SourceFile],
    ) -> Result<(), super::Diagnostic> {
        for source in sources {
            let Ok((arena, roots)) = crate::reader::read_file(&source.contents) else {
                continue;
            };
            if !crate::macrobuild::is_macro_library(&arena, &roots) {
                continue;
            }
            let info = crate::wit::collect(&arena, &roots)
                .map_err(|e| super::Diagnostic::new(super::Phase::Wit, Some(&source.name), e))?;
            let bytes = crate::macrobuild::build_macro_component(&arena, &roots)
                .map_err(|e| super::Diagnostic::new(super::Phase::Emit, Some(&source.name), e))?;
            self.project_libraries
                .insert(info.package_path, (info.package, bytes));
        }
        Ok(())
    }

    fn import_source(&self, import: &ImportInfo) -> MacroSource {
        if import.from.is_none()
            && let Some((package, bytes)) = self.project_libraries.get(&import.package)
            && (import.version.is_none() || *package == package_id(import))
        {
            return MacroSource::Component(bytes.clone());
        }
        MacroSource::Imported(MacroImport {
            package_id: package_id(import),
            source_path: import.from.clone(),
        })
    }

    fn require(&mut self, key: String, source: MacroSource) -> Result<Manifest, String> {
        if let Some(manifest) = self.manifests.get(&key) {
            return Ok(manifest.clone());
        }
        match self.host.manifest(&key) {
            Some(result) => {
                let manifest = result?;
                let mut names = HashSet::new();
                for (name, _) in &manifest {
                    if name.is_empty() || !names.insert(name) {
                        return Err("macro manifest contains empty or repeated names".into());
                    }
                }
                self.manifests.insert(key, manifest.clone());
                Ok(manifest)
            }
            None => {
                if !self.pending.iter().any(|request| request.key == key) {
                    self.pending.push(MacroRequest { key, source });
                }
                Err("macro capability required".into())
            }
        }
    }

    pub(super) fn read(&mut self, source: &str) -> Result<(Arena, Vec<NodeId>), ReadError> {
        let mut table = MacroTable::core();
        let mut hook = |arena: &Arena, id: NodeId, table: &mut MacroTable| {
            let Some(import) = macro_import(arena, id) else {
                return Ok(());
            };
            let key = import_key(&import);
            let source = self.import_source(&import);
            let manifest = self.require(key, source).map_err(|msg| ReadError {
                msg,
                at: arena.span(id).0,
            })?;
            for (name, arity) in manifest {
                table.register_foreign(&import.alias, format!("{name}-MACRO"), arity as usize);
            }
            Ok(())
        };
        crate::reader::read_with_hook(source, &mut table, Some(&mut hook))
    }

    pub(super) fn prepare_local(&mut self, arena: &Arena, roots: &[NodeId]) -> Result<(), String> {
        if roots.iter().any(|id| local_macro(arena, *id).is_some()) {
            let bytes = crate::emit::emit_macro_component(arena, roots)?;
            let key = format!("local:{:x}", Sha256::digest(&bytes));
            self.require(key, MacroSource::Component(bytes))?;
        }
        Ok(())
    }

    pub(super) fn expand(
        &mut self,
        arena: Arena,
        roots: &[NodeId],
    ) -> Result<(Arena, Vec<NodeId>), String> {
        let imports = roots
            .iter()
            .filter_map(|id| macro_import(&arena, *id))
            .collect::<Vec<_>>();
        let locals = roots
            .iter()
            .filter_map(|id| local_macro(&arena, *id))
            .collect::<HashSet<_>>();
        let local_key = if locals.is_empty() {
            None
        } else {
            let bytes = crate::emit::emit_macro_component(&arena, roots)?;
            let key = format!("local:{:x}", Sha256::digest(&bytes));
            self.require(key.clone(), MacroSource::Component(bytes))?;
            Some(key)
        };
        let mut expander = Expander {
            catalog: self,
            imports,
            locals,
            local_key,
        };
        crate::expand::expand_file(arena, roots, Some(&mut expander))
    }
}

struct Expander<'a, 'b> {
    catalog: &'a mut Catalog<'b>,
    imports: Vec<ImportInfo>,
    locals: HashSet<String>,
    local_key: Option<String>,
}

impl Expander<'_, '_> {
    fn owner(&self, alias: Option<&str>, name: &str) -> Option<(String, String)> {
        if alias.is_none() && self.locals.contains(name) {
            return self.local_key.clone().map(|key| (key, name.to_owned()));
        }
        let qualified = self
            .imports
            .iter()
            .filter_map(|import| {
                name.strip_prefix(&format!("{}-", import.alias.to_ascii_lowercase()))
                    .map(|suffix| (import.alias.as_str(), suffix))
            })
            .max_by_key(|(alias, _)| alias.len());
        let (alias, name) = match (alias, qualified) {
            (Some(alias), _) => (Some(alias), name),
            (None, Some((alias, name))) => (Some(alias), name),
            _ => (None, name),
        };
        self.imports
            .iter()
            .filter(|import| alias.is_none_or(|alias| alias == import.alias))
            .find_map(|import| {
                let key = import_key(import);
                self.catalog
                    .manifests
                    .get(&key)?
                    .iter()
                    .any(|(candidate, _)| candidate == name)
                    .then(|| (key, name.to_owned()))
            })
    }
}

impl crate::expand::ForeignExpander for Expander<'_, '_> {
    fn expand_call(
        &mut self,
        alias: Option<&str>,
        name: &str,
        arena: &Arena,
        call_id: NodeId,
    ) -> Option<Result<(Arena, NodeId), String>> {
        let (key, name) = self.owner(alias, name)?;
        let args = crate::meta::arena_to_tree(arena, call_id);
        Some(
            self.catalog
                .host
                .expand(&key, &name, &args)
                .and_then(|tree| {
                    crate::meta::validate_tree(&tree)?;
                    Ok(crate::meta::tree_to_arena(&tree))
                }),
        )
    }
}

fn macro_import(arena: &Arena, id: NodeId) -> Option<ImportInfo> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    if !matches!(items.first().map(|id| arena.node(*id)), Some(Node::Sym(head)) if head == "import-MACRO")
    {
        return None;
    }
    let Node::Rec(fields) = arena.node(*items.get(1)?) else {
        return None;
    };
    let import = crate::wit::import_from_record(arena, fields).ok()?;
    import.macros.then_some(import)
}

fn local_macro(arena: &Arena, id: NodeId) -> Option<String> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    if items.len() != 4
        || !matches!(arena.node(items[0]), Node::Sym(head) if head == "defmacro-MACRO")
    {
        return None;
    }
    match arena.node(items[1]) {
        Node::Sym(name) => Some(name.clone()),
        _ => None,
    }
}

fn package_id(import: &ImportInfo) -> String {
    match &import.version {
        Some(version) => format!("{}@{version}", import.package),
        None => import.package.clone(),
    }
}

fn import_key(import: &ImportInfo) -> String {
    format!("import:{:?}:{:?}", package_id(import), import.from)
}
