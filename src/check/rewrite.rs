use super::*;

pub fn resolve_overloads(arena: Arena, roots: &[NodeId]) -> Result<(Arena, Vec<NodeId>), String> {
    resolve_overloads_excluding(arena, roots, &std::collections::HashSet::new())
}

pub fn resolve_overloads_excluding(
    arena: Arena,
    roots: &[NodeId],
    keep: &std::collections::HashSet<String>,
) -> Result<(Arena, Vec<NodeId>), String> {
    let checker = Checker::collect(&arena, roots);
    checker.check_roots(roots)?;

    let mut overloads = checker.overload_names();
    overloads.retain(|n| !keep.contains(n));
    if overloads.is_empty() && checker.builtin_callbacks.borrow().is_empty() {
        return Ok((arena, roots.to_vec()));
    }

    let resolved = checker.resolved.into_inner();
    let builtin_callbacks = checker.builtin_callbacks.into_inner();
    let mut rw = Rewriter {
        arena: &arena,
        overloads: &overloads,
        resolved: &resolved,
        builtin_callbacks: &builtin_callbacks,
        out: Arena::new(),
        def_counts: HashMap::new(),
    };
    let new_roots: Vec<NodeId> = roots.iter().map(|&r| rw.rewrite_root(r)).collect();
    Ok((rw.out, new_roots))
}

fn mangled_def_name(name: &str, k: usize) -> String {
    format!("{name}${k}")
}

struct Rewriter<'a> {
    arena: &'a Arena,
    overloads: &'a std::collections::HashSet<String>,
    resolved: &'a HashMap<NodeId, usize>,
    builtin_callbacks: &'a HashMap<NodeId, usize>,
    out: Arena,

    def_counts: HashMap<String, usize>,
}

impl<'a> Rewriter<'a> {
    fn rewrite_root(&mut self, id: NodeId) -> NodeId {
        if let Some((name, _expr)) = as_def(self.arena, id)
            && self.overloads.contains(name)
        {
            let name = name.to_string();
            let Node::Tup(items) = self.arena.node(id) else {
                unreachable!("as_def matched a Tup")
            };
            let items = items.clone();
            let k = self.def_counts.entry(name.clone()).or_insert(0);
            let unique = mangled_def_name(&name, *k);
            *k += 1;
            let span = self.arena.span(id);

            let head = self.rewrite(items[0]);
            let new_name = self.out.add(Node::Sym(unique), self.arena.span(items[1]));
            let expr = self.rewrite(items[2]);
            return self.out.add(Node::Tup(vec![head, new_name, expr]), span);
        }
        self.rewrite(id)
    }

    fn builtin_callback(&mut self, id: NodeId, arity: usize) -> NodeId {
        let span = self.arena.span(id);
        let names = (0..arity)
            .map(|index| format!("builtin-argument-{index}"))
            .collect::<Vec<_>>();
        let params = self.out.add(Node::Flg(names.clone()), span);
        let mut call = vec![self.out.add(self.arena.node(id).clone(), span)];
        call.extend(
            names
                .into_iter()
                .map(|name| self.out.add(Node::Sym(name), span)),
        );
        let body = self.out.add(Node::Tup(call), span);
        let head = self.out.add(Node::Sym("fn-MACRO".into()), span);
        self.out.add(Node::Tup(vec![head, params, body]), span)
    }

    fn rewrite(&mut self, id: NodeId) -> NodeId {
        let span = self.arena.span(id);
        if let Some(&arity) = self.builtin_callbacks.get(&id) {
            return self.builtin_callback(id, arity);
        }
        match self.arena.node(id).clone() {
            Node::Tup(items) => {
                if let Some(&chosen) = self.resolved.get(&id)
                    && let Some(&head) = items.first()
                    && let Node::Sym(name) = self.arena.node(head)
                    && self.overloads.contains(name)
                {
                    let unique = mangled_def_name(name, chosen);
                    let new_head = self.out.add(Node::Sym(unique), self.arena.span(head));
                    let mut kids = Vec::with_capacity(items.len());
                    kids.push(new_head);
                    for &x in &items[1..] {
                        kids.push(self.rewrite(x));
                    }
                    return self.out.add(Node::Tup(kids), span);
                }
                let kids: Vec<NodeId> = items.iter().map(|&x| self.rewrite(x)).collect();
                self.out.add(Node::Tup(kids), span)
            }
            Node::Lst(items) => {
                let kids: Vec<NodeId> = items.iter().map(|&x| self.rewrite(x)).collect();
                self.out.add(Node::Lst(kids), span)
            }
            Node::Rec(fields) => {
                let nf: Vec<(String, NodeId)> = fields
                    .iter()
                    .map(|(k, v)| (k.clone(), self.rewrite(*v)))
                    .collect();
                self.out.add(Node::Rec(nf), span)
            }
            leaf => self.out.add(leaf, span),
        }
    }
}
