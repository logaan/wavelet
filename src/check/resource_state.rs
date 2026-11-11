use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Data,
    Cell,
    Unknown,
}

struct StateChecker<'a> {
    arena: &'a Arena,
    defs: HashMap<String, NodeId>,
    resources: std::collections::HashSet<String>,
    cases: std::collections::HashSet<String>,
    visiting: std::collections::HashSet<NodeId>,
}

pub(super) fn validate(arena: &Arena, roots: &[NodeId]) -> Result<(), String> {
    let resources = resource_names(arena, roots);
    if resources.is_empty() {
        return Ok(());
    }
    let mut checker = StateChecker {
        arena,
        defs: roots
            .iter()
            .filter_map(|&id| as_def(arena, id).map(|(n, e)| (n.to_owned(), e)))
            .collect(),
        resources,
        cases: roots
            .iter()
            .filter_map(|&id| as_deftype(arena, id))
            .flat_map(|(_, ty)| {
                if let TypeDef::Variant(cases) = ty {
                    cases.into_iter().map(|(name, _)| name).collect()
                } else {
                    Vec::new()
                }
            })
            .collect(),
        visiting: Default::default(),
    };
    for &root in roots {
        if let Some((name, members)) = as_defresource(arena, root) {
            for (member, &node) in members.iter().map(|(n, v)| (n, v)) {
                let (_, function) = strip_static(arena, node);
                let Some(params) = fn_params(arena, function) else {
                    continue;
                };
                let mut scope = params
                    .into_iter()
                    .map(|(n, t)| (n, checker.type_shape(&t)))
                    .collect();
                let shape = checker.expr(fn_body(arena, function).unwrap(), &mut scope)?;
                if member == "New" && shape == Shape::Unknown {
                    return Err(format!(
                        "resource `{name}` state must be plain data or one cell containing plain data; nested or shared cells cannot be persisted safely"
                    ));
                }
            }
        } else if let Some((_, expr)) = as_def(arena, root) {
            if let Some(params) = fn_params(arena, expr) {
                let mut scope = params
                    .into_iter()
                    .map(|(n, t)| (n, checker.type_shape(&t)))
                    .collect();
                checker.expr(fn_body(arena, expr).unwrap(), &mut scope)?;
            } else {
                checker.expr(expr, &mut HashMap::new())?;
            }
        } else {
            checker.expr(root, &mut HashMap::new())?;
        }
    }
    Ok(())
}

impl StateChecker<'_> {
    fn type_shape(&self, ty: &Type) -> Shape {
        match ty {
            Type::Unknown | Type::Fn(..) | Type::Cell(_) => Shape::Unknown,
            Type::Named(name) if self.resources.contains(name) => Shape::Unknown,
            Type::List(inner) | Type::Option(inner) => self.type_shape(inner),
            Type::Tuple(types) => Self::all_data(types.iter().map(|t| self.type_shape(t))),
            Type::Record(fields) => Self::all_data(fields.iter().map(|(_, t)| self.type_shape(t))),
            Type::Result(ok, err) => Self::all_data([self.type_shape(ok), self.type_shape(err)]),
            _ => Shape::Data,
        }
    }

    fn all_data(shapes: impl IntoIterator<Item = Shape>) -> Shape {
        if shapes.into_iter().all(|s| s == Shape::Data) {
            Shape::Data
        } else {
            Shape::Unknown
        }
    }

    fn expr(&mut self, id: NodeId, scope: &mut HashMap<String, Shape>) -> Result<Shape, String> {
        match self.arena.node(id).clone() {
            Node::Sym(name) => Ok(scope.get(&name).copied().unwrap_or_else(|| {
                if name == "none" || name == "pi" {
                    Shape::Data
                } else {
                    Shape::Unknown
                }
            })),
            Node::Lst(items) => {
                let values = items
                    .into_iter()
                    .map(|i| self.expr(i, scope))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::all_data(values))
            }
            Node::Rec(fields) => {
                let values = fields
                    .into_iter()
                    .map(|(_, i)| self.expr(i, scope))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::all_data(values))
            }
            Node::Tup(items) if !items.is_empty() => {
                let Node::Sym(name) = self.arena.node(items[0]) else {
                    return Ok(Shape::Unknown);
                };
                let name = name.clone();
                let args = &items[1..];
                match name.as_str() {
                    "quote-MACRO" | "quasi-MACRO" => return Ok(Shape::Data),
                    "the-MACRO" if args.len() == 2 => return self.expr(args[1], scope),
                    "let-MACRO" if args.len() == 2 => {
                        let mut inner = scope.clone();
                        if let Node::Rec(fields) = self.arena.node(args[0]).clone() {
                            for (name, value) in fields {
                                let shape = self.expr(value, &mut inner)?;
                                inner.insert(name, shape);
                            }
                        }
                        return self.expr(args[1], &mut inner);
                    }
                    "match-MACRO" if args.len() == 2 => {
                        self.expr(args[0], scope)?;
                        let Node::Lst(clauses) = self.arena.node(args[1]).clone() else {
                            return Ok(Shape::Unknown);
                        };
                        let mut result = None;
                        for clause in clauses {
                            if let Node::Tup(pair) = self.arena.node(clause).clone()
                                && pair.len() == 2
                            {
                                let shape = self.expr(pair[1], &mut scope.clone())?;
                                result = Some(match result {
                                    None => shape,
                                    Some(previous) if previous == shape => shape,
                                    _ => Shape::Unknown,
                                });
                            }
                        }
                        return Ok(result.unwrap_or(Shape::Unknown));
                    }
                    "fn-MACRO" => return Ok(Shape::Unknown),
                    _ => {}
                }
                let values = args
                    .iter()
                    .map(|&i| self.expr(i, scope))
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(&function) = self.defs.get(&name) {
                    let Some(params) = fn_params(self.arena, function) else {
                        return Ok(Shape::Unknown);
                    };
                    if !self.visiting.insert(function) {
                        return Ok(Shape::Unknown);
                    }
                    let mut inner = params.into_iter().map(|(n, _)| n).zip(values).collect();
                    let result = self.expr(fn_body(self.arena, function).unwrap(), &mut inner);
                    self.visiting.remove(&function);
                    return result;
                }
                if self.cases.contains(&name) {
                    return Ok(Self::all_data(values));
                }
                match name.as_str() {
                    "cell-new" if values.len() == 1 => {
                        if values[0] != Shape::Data {
                            return Err("persistent cells may contain only statically known plain data; nested cells, aliases, cycles, and function values are unsupported".into());
                        }
                        Ok(Shape::Cell)
                    }
                    "cell-set" if values.len() == 2 => {
                        if values[1] != Shape::Data {
                            return Err("cell-set in a resource component requires plain data; storing cells could create sharing or cycles".into());
                        }
                        Ok(Shape::Data)
                    }
                    "cell-get" => Ok(Shape::Data),
                    "add" | "sub" | "mul" | "div" | "rem" | "neg" | "abs" | "eq" | "lt" | "le"
                    | "gt" | "ge" | "not" | "len" | "empty" | "to-string" | "str-cat" | "upper"
                    | "lower" | "contains" | "form-kind" => Ok(Shape::Data),
                    name if name.starts_with("to-") => Ok(Shape::Data),
                    name if crate::builtins::NAMES.contains(&name) => Ok(Self::all_data(values)),
                    _ => Ok(Shape::Unknown),
                }
            }
            _ => Ok(Shape::Data),
        }
    }
}
