use super::*;
use std::collections::HashSet;

const MESSAGE: &str = "eval error: same-component ownership transfer is unsupported; use borrow(resource) for local calls and callbacks, and transfer owned handles only across component boundaries";

pub(super) fn validate(checker: &Checker<'_>, roots: &[NodeId]) -> Result<(), String> {
    if checker.resource_types.is_empty() {
        return Ok(());
    }
    let mut owned = HashSet::new();
    for &root in roots {
        if let Some((name, function)) = as_def(checker.arena, root)
            && let Some((params, _)) = as_fn(checker.arena, function)
        {
            let declared = export_params(checker.arena, roots, name);
            let inferred = checker.sigs.get(name).and_then(|sigs| {
                sigs.iter()
                    .find(|sig| sig.body == fn_body(checker.arena, function))
            });
            if owns_parameters(
                checker,
                params,
                declared,
                inferred.map(|sig| sig.params.as_slice()),
                false,
            ) {
                owned.insert(name.to_owned());
            }
        }
        if let Some((resource, members)) = as_defresource(checker.arena, root) {
            for (name, node) in members {
                let (is_static, function) = strip_static(checker.arena, *node);
                let Some((params, _)) = as_fn(checker.arena, function) else {
                    continue;
                };
                if name != "Drop"
                    && owns_parameters(checker, params, None, None, name != "New" && !is_static)
                {
                    owned.insert(if name == "New" {
                        resource.to_owned()
                    } else {
                        format!("{resource}-{name}")
                    });
                }
            }
        }
    }
    let walk = OwnershipWalk { checker, owned };
    for &root in roots {
        walk.root(root)?;
    }
    Ok(())
}

fn export_params(arena: &Arena, roots: &[NodeId], function: &str) -> Option<NodeId> {
    roots.iter().find_map(|&root| {
        let Node::Tup(items) = arena.node(root) else {
            return None;
        };
        if items.len() != 2
            || !matches!(arena.node(items[0]), Node::Sym(name) if name == "export-MACRO")
        {
            return None;
        }
        let Node::Rec(fields) = arena.node(items[1]) else {
            return None;
        };
        if !fields.iter().any(|(key, value)| {
            key == "name" && matches!(arena.node(*value), Node::Sym(name) if name == function)
        }) {
            return None;
        }
        fields
            .iter()
            .find_map(|(key, value)| (key == "params").then_some(*value))
    })
}

fn owns_parameters(
    checker: &Checker<'_>,
    params: NodeId,
    exported: Option<NodeId>,
    inferred: Option<&[(String, Type)]>,
    skip_receiver: bool,
) -> bool {
    let parameters = parse_params(checker.arena, params);
    parameters
        .iter()
        .enumerate()
        .skip(usize::from(skip_receiver))
        .any(|(index, (name, ty))| {
            let annotation = [Some(params), exported]
                .into_iter()
                .flatten()
                .find_map(|node| {
                    let Node::Rec(fields) = checker.arena.node(node) else {
                        return None;
                    };
                    fields
                        .iter()
                        .find_map(|(key, value)| (key == name).then_some(*value))
                });
            match annotation {
                Some(node) => owns_type_form(checker, node),
                None => has_resource(
                    checker,
                    inferred
                        .and_then(|params| params.get(index))
                        .map(|(_, ty)| ty)
                        .unwrap_or(ty),
                    &mut HashSet::new(),
                ),
            }
        })
}

fn owns_type_form(checker: &Checker<'_>, node: NodeId) -> bool {
    match checker.arena.node(node) {
        Node::Tup(items) if !items.is_empty() => {
            if matches!(checker.arena.node(items[0]), Node::Sym(name) if name == "borrow") {
                return false;
            }
            items[1..].iter().any(|&node| owns_type_form(checker, node))
        }
        Node::Rec(fields) => fields
            .iter()
            .any(|(_, node)| owns_type_form(checker, *node)),
        _ => has_resource(
            checker,
            &Type::from_form(checker.arena, node),
            &mut HashSet::new(),
        ),
    }
}

fn has_resource(checker: &Checker<'_>, ty: &Type, visiting: &mut HashSet<String>) -> bool {
    match ty {
        Type::Named(name) if checker.resource_types.contains(name) => true,
        Type::Named(name) if visiting.insert(name.clone()) => match checker.types.get(name) {
            Some(TypeDef::Alias(ty)) => has_resource(checker, ty, visiting),
            Some(TypeDef::Record(fields)) => fields
                .iter()
                .any(|(_, ty)| has_resource(checker, ty, visiting)),
            Some(TypeDef::Variant(cases)) => cases
                .iter()
                .any(|(_, types)| types.iter().any(|ty| has_resource(checker, ty, visiting))),
            _ => false,
        },
        Type::List(ty) | Type::Option(ty) | Type::Cell(ty) => has_resource(checker, ty, visiting),
        Type::Tuple(types) => types.iter().any(|ty| has_resource(checker, ty, visiting)),
        Type::Record(fields) => fields
            .iter()
            .any(|(_, ty)| has_resource(checker, ty, visiting)),
        Type::Result(ok, error) => {
            has_resource(checker, ok, visiting) || has_resource(checker, error, visiting)
        }
        _ => false,
    }
}

struct OwnershipWalk<'a, 'b> {
    checker: &'a Checker<'b>,
    owned: HashSet<String>,
}

impl OwnershipWalk<'_, '_> {
    fn root(&self, node: NodeId) -> Result<(), String> {
        if let Some((_, expr)) = as_def(self.checker.arena, node)
            && let Some((params, body)) = as_fn(self.checker.arena, expr)
        {
            return self.expr(body, &self.parameter_names(params));
        }
        if let Some((_, members)) = as_defresource(self.checker.arena, node) {
            for (_, node) in members {
                let (_, function) = strip_static(self.checker.arena, *node);
                if let Some((params, body)) = as_fn(self.checker.arena, function) {
                    self.expr(body, &self.parameter_names(params))?;
                }
            }
            return Ok(());
        }
        self.expr(node, &HashSet::new())
    }

    fn parameter_names(&self, params: NodeId) -> HashSet<String> {
        parse_params(self.checker.arena, params)
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    fn expr(&self, node: NodeId, shadowed: &HashSet<String>) -> Result<(), String> {
        match self.checker.arena.node(node) {
            Node::Sym(name) if self.owned.contains(name) && !shadowed.contains(name) => {
                Err(MESSAGE.into())
            }
            Node::Tup(items) if !items.is_empty() => self.call(items, shadowed),
            Node::Lst(items) => {
                for &node in items {
                    self.expr(node, shadowed)?;
                }
                Ok(())
            }
            Node::Rec(fields) => {
                for (_, node) in fields {
                    self.expr(*node, shadowed)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn call(&self, items: &[NodeId], shadowed: &HashSet<String>) -> Result<(), String> {
        let Node::Sym(name) = self.checker.arena.node(items[0]) else {
            for &node in items {
                self.expr(node, shadowed)?;
            }
            return Ok(());
        };
        match name.as_str() {
            "package-MACRO" | "import-MACRO" | "export-MACRO" | "deftype-MACRO"
            | "instantiate-MACRO" | "quote-MACRO" | "defmacro-MACRO" => return Ok(()),
            "quasi-MACRO" if items.len() == 2 => return self.quasi_holes(items[1], 1, shadowed),
            "the-MACRO" if items.len() == 3 => return self.expr(items[2], shadowed),
            "def-MACRO" if items.len() == 3 => return self.expr(items[2], shadowed),
            "fn-MACRO" if items.len() == 3 => {
                if owns_parameters(self.checker, items[1], None, None, false) {
                    return Err(MESSAGE.into());
                }
                let mut inner = shadowed.clone();
                inner.extend(self.parameter_names(items[1]));
                return self.expr(items[2], &inner);
            }
            "let-MACRO" if items.len() == 3 => return self.let_form(items, shadowed),
            "match-MACRO" if items.len() == 3 => return self.match_form(items, shadowed),
            _ if name.ends_with("-MACRO") => return Ok(()),
            _ => {}
        }
        for &node in items {
            self.expr(node, shadowed)?;
        }
        Ok(())
    }

    fn quasi_holes(
        &self,
        node: NodeId,
        depth: usize,
        shadowed: &HashSet<String>,
    ) -> Result<(), String> {
        match self.checker.arena.node(node) {
            Node::Tup(items) => {
                if items.len() == 2
                    && let Node::Sym(name) = self.checker.arena.node(items[0])
                {
                    if name == "quasi-MACRO" {
                        return self.quasi_holes(items[1], depth + 1, shadowed);
                    }
                    if name == "unquote-MACRO" || name == "splice-MACRO" {
                        return if depth == 1 {
                            self.expr(items[1], shadowed)
                        } else {
                            self.quasi_holes(items[1], depth - 1, shadowed)
                        };
                    }
                }
                for &node in items {
                    self.quasi_holes(node, depth, shadowed)?;
                }
            }
            Node::Lst(items) => {
                for &node in items {
                    self.quasi_holes(node, depth, shadowed)?;
                }
            }
            Node::Rec(fields) => {
                for (_, node) in fields {
                    self.quasi_holes(*node, depth, shadowed)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn let_form(&self, items: &[NodeId], shadowed: &HashSet<String>) -> Result<(), String> {
        let mut inner = shadowed.clone();
        if let Node::Rec(fields) = self.checker.arena.node(items[1]) {
            for (name, value) in fields {
                self.expr(*value, &inner)?;
                inner.insert(name.clone());
            }
        }
        self.expr(items[2], &inner)
    }

    fn match_form(&self, items: &[NodeId], shadowed: &HashSet<String>) -> Result<(), String> {
        self.expr(items[1], shadowed)?;
        if let Node::Lst(clauses) = self.checker.arena.node(items[2]) {
            for &clause in clauses {
                if let Node::Tup(pair) = self.checker.arena.node(clause)
                    && pair.len() == 2
                {
                    let mut inner = shadowed.clone();
                    self.pattern_names(pair[0], &mut inner);
                    self.expr(pair[1], &inner)?;
                }
            }
        }
        Ok(())
    }

    fn pattern_names(&self, node: NodeId, names: &mut HashSet<String>) {
        match self.checker.arena.node(node) {
            Node::Sym(name) => {
                names.insert(name.clone());
            }
            Node::Tup(items) => {
                let is_case = items.first().is_some_and(|&head| matches!(self.checker.arena.node(head), Node::Sym(name) if matches!(name.as_str(), "some" | "none" | "ok" | "err") || self.checker.types.values().any(|ty| matches!(ty, TypeDef::Variant(cases) if cases.iter().any(|(case, _)| case == name)))));
                for &node in items.iter().skip(usize::from(is_case)) {
                    self.pattern_names(node, names);
                }
            }
            Node::Lst(items) => {
                for &node in items {
                    self.pattern_names(node, names);
                }
            }
            Node::Rec(fields) => {
                for (_, node) in fields {
                    self.pattern_names(*node, names);
                }
            }
            _ => {}
        }
    }
}
