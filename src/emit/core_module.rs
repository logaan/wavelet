//! Per-file core wasm module assembly: `emit_core_module` wires imports,
//! functions, exports, and data into a `wasm_encoder::Module`; plus the 5.8
//! let-lambda devirtualization pre-scan.

use super::*;

// --------------------------------------------------------- helper bodies

/// 5.8 pre-scan: collect the `Fn`-literal nodes that qualify for capture
/// devirtualization — an inline `Fn` bound directly as a `Let` binding init
/// whose checked type is a fully-concrete arrow (`Type::Fn`). Walks a def body
/// in deterministic pre-order; the caller reserves one core-function index per
/// returned node (in this order) so the lifted bodies land at known indices.
///
/// Only nodes whose enclosing def is actually emitted are scanned (the caller
/// passes internal-def bodies), so every reserved index gets exactly one body —
/// no dangling function declarations. A qualifying lambda the scan misses (in
/// an overload/value/resource body) simply falls back to the boxed closure.
pub(crate) fn scan_let_lambdas(
    arena: &Arena,
    id: NodeId,
    node_types: &crate::check::NodeTypes,
    out: &mut Vec<NodeId>,
) {
    let is_fn_arrow = |n: NodeId| -> bool {
        matches!(arena.node(n), Node::Tup(items)
            if !items.is_empty()
                && matches!(arena.node(items[0]), Node::Sym(s) if s == "fn-MACRO"))
            && matches!(node_types.get(&n), Some(crate::check::Type::Fn(..)))
    };
    if let Node::Tup(items) = arena.node(id)
        && items.len() >= 3
        && matches!(arena.node(items[0]), Node::Sym(s) if s == "let-MACRO")
        && let Node::Rec(fields) = arena.node(items[1])
    {
        for (_, v) in fields {
            if is_fn_arrow(*v) {
                out.push(*v);
            }
        }
    }
    match arena.node(id) {
        Node::Tup(items) | Node::Lst(items) => {
            for &c in items {
                scan_let_lambdas(arena, c, node_types, out);
            }
        }
        Node::Rec(fields) => {
            for (_, c) in fields {
                scan_let_lambdas(arena, *c, node_types, out);
            }
        }
        _ => {}
    }
}

pub(crate) fn emit_core_module(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    let feats = features_of(arena, info);

    // Per-node static types (goal 5). Reconstruct the same import-signature
    // table the build step feeds the checker (`build.rs`), so this table
    // matches what checking saw — including qualified-call result types. A
    // program that fails checking yields an *empty* table (boxed fallback
    // everywhere) rather than a second error surface: the build path has
    // already rejected ill-typed programs before emitting.
    let node_types_tbl = {
        let mut import_sigs = crate::check::ImportSigs::new();
        for imp in &info.imports {
            if crate::wit::is_macro_only(imp) {
                continue;
            }
            if let Some(dep) = deps.get(&imp.package) {
                let iface = imp.path.split_once('/').map(|(_, i)| i).unwrap_or("api");
                import_sigs.extend(crate::wit::import_sigs_for(&imp.alias, &dep.funcs, iface));
            }
        }
        import_sigs.extend(crate::wit::functor_import_sigs(&info.functors));
        crate::check::node_types_with_imports(arena, roots, &import_sigs).unwrap_or_default()
    };

    // named types in scope: this file's own DefTypes, plus those of every dep.
    // Records resolve via `records`; enum/variant/flags (dep-only today) via
    // `defs`.
    let mut type_env = TypeEnv::default();
    for (name, fields) in record_types(arena, &info.types) {
        type_env.records.insert(name, fields);
    }
    // Local non-record `DefType`s: variants/flags as `defs`, list/tuple/option/
    // result/alias names as `aliases`. This lets a functor instantiate over a
    // named compound element (`DefType pair list<s32>` → `pair-set`) and lets an
    // export pass/return those types — matching the interpreter, which already
    // handles every element kind structurally.
    let (local_defs, local_aliases) = local_non_record_types(arena, &info.types);
    // The file's own variant/enum cases, for bare case construction (4.1).
    let mut local_cases: HashMap<String, bool> = HashMap::new();
    for (_name, def) in &local_defs {
        match def {
            TypeDef::Enum(cases) => {
                for c in cases {
                    local_cases.insert(c.clone(), false);
                }
            }
            TypeDef::Variant(cases) => {
                for (c, p) in cases {
                    local_cases.insert(c.clone(), p.is_some());
                }
            }
            _ => {}
        }
    }
    for (name, def) in local_defs {
        type_env.defs.insert(name, def);
    }
    for (name, target) in local_aliases {
        type_env.aliases.insert(name, target);
    }
    for dep in deps.values() {
        for (name, fields) in &dep.types {
            type_env
                .records
                .entry(name.clone())
                .or_insert_with(|| fields.clone());
        }
        for (name, def) in &dep.type_defs {
            type_env
                .defs
                .entry(name.clone())
                .or_insert_with(|| def.clone());
        }
        for (name, target) in &dep.aliases {
            type_env
                .aliases
                .entry(name.clone())
                .or_insert_with(|| target.clone());
        }
    }
    // Each functor instantiation exports a `set` resource. Declaring `set` as a
    // `TypeDef::Resource` makes `wit_ty` map a bare `set` (and `own<set>` /
    // `borrow<set>`) to `WitTy::Handle` at the boundary — `emit.rs:177`–`184`,
    // `:260`. All instantiations name their resource `set` (interfaces differ:
    // `point-set`, `string-set`, …); since the element type is the only thing
    // that varies and the rep is element-generic, one `set -> Resource` entry
    // serves every instantiation. The element type itself resolves through the
    // same `type_env` (records via `type_env.records`, primitives intrinsically).
    for inst in &info.functors {
        // The bare resource name, as it appears in a method signature
        // (`add: func(value: point)` → receiver `set`).
        type_env
            .defs
            .entry("set".to_string())
            .or_insert(TypeDef::Resource);
        // The dotted `<iface>.set` form is the *return type text* an export body
        // gets for `alias/new()` (see `wit::functor_op_table`: a `Handle` op
        // infers to `"{iface}.set"`). Register it as a resource too so the export
        // wrapper's `flat_result`/`wit_ty` map it to `WitTy::Handle` rather than
        // rejecting it as an unknown type.
        type_env
            .defs
            .entry(format!("{}.set", inst.iface))
            .or_insert(TypeDef::Resource);
    }

    // User-declared resources (4.5): register each resource NAME as a boundary
    // resource type so `wit_ty` maps `counter` / `own(counter)` / `borrow(counter)`
    // to a handle, exactly as for the functor `set`. The classified member forms
    // drive index reservation, body emission, call routing, and boundary exports.
    let user_res_forms = user_resource_forms(arena, roots)?;
    for rf in &user_res_forms {
        type_env
            .defs
            .entry(rf.name.clone())
            .or_insert(TypeDef::Resource);
    }

    let mut em = Emitter {
        arena,
        info,
        deps,
        type_env,
        local_cases,
        data: Vec::new(),
        str_cache: HashMap::new(),
        types: Vec::new(),
        imports: Vec::new(),
        import_fn: HashMap::new(),
        h: Helpers::default(),
        funcs: HashMap::new(),
        value_globals: HashMap::new(),
        compiling_values: Vec::new(),
        bodies: Vec::new(),
        closure_bodies: Vec::new(),
        known_fn_names: Vec::new(),
        known_lambdas: Vec::new(),
        lambda_reserved: HashMap::new(),
        lambda_order: Vec::new(),
        lambda_stash: HashMap::new(),
        fn_wrappers: HashMap::new(),
        fn_box_cache: HashMap::new(),
        var_box_cache: HashMap::new(),
        false_addr: 0,
        true_addr: 0,
        macro_expand_idx: None,
        functor_fns: HashMap::new(),
        user_res: HashMap::new(),
        node_types: node_types_tbl,
        mem_tys: Vec::new(),
    };

    // static boxes: false @16, true @24
    em.false_addr = DATA_BASE;
    em.put_i32(TAG_BOOL);
    em.put_i32(0);
    em.true_addr = DATA_BASE + 8;
    em.put_i32(TAG_BOOL);
    em.put_i32(1);

    // ---- imports (function index space starts here)
    let mut n_imports = 0u32;
    let mut add_import =
        |em: &mut Emitter, module: &str, field: &str, p: Vec<ValType>, r: Vec<ValType>| {
            let t = em.ty_idx(p, r);
            em.imports.push((module.to_string(), field.to_string(), t));
            em.import_fn
                .insert((module.to_string(), field.to_string()), n_imports);
            n_imports += 1;
        };

    use ValType::{F64, I32, I64};
    let _ = (I32, I64, F64);
    for (alias, fname) in &feats.dep_calls {
        // A functor alias (`pts` in `pts/new`) is NOT a runtime import — its
        // `set` resource is *exported*, not imported, so it contributes no import
        // here. Routing the call itself is step 04; the body's `dep_call` stubs it
        // for now. Skip it so the import-declaration loop does not reject it as an
        // unknown import alias.
        if info.functors.iter().any(|f| &f.alias == alias) {
            continue;
        }
        // A user-resource method/static (`counter/next`, `counter/sum`) is an
        // exported resource member, not a runtime import; `dep_call` routes it to
        // the emitted member fn. Skip it here for the same reason as a functor.
        if user_res_forms.iter().any(|rf| &rf.name == alias) {
            continue;
        }
        let imp = info
            .imports
            .iter()
            .find(|i| &i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        let dep = deps.get(&imp.package).ok_or(format!(
            "dependency `{}` is not in the build set",
            imp.package
        ))?;
        let iface = import_iface(&imp.path);
        // Same op-name resolution as `dep_call`, so a resource operation's
        // core import is declared under its mangled WIT name (`sig.name`).
        // A dep *case constructor* call (`t/circle(…)`, 4.1) is not a runtime
        // import — the variant box is built locally — so it declares nothing.
        let sig = match resolve_dep_func(dep, &iface, fname) {
            Ok(sig) => sig,
            Err(e) => {
                if dep_case(dep, fname).is_some() {
                    continue;
                }
                return Err(e);
            }
        };
        let mut p = Vec::new();
        for (_, t) in &sig.params {
            p.extend_from_slice(&flat_checked(&wit_ty(t, &em.type_env)?)?);
        }
        let r = match flat_result(sig, &em.type_env)? {
            FlatRes::None => vec![],
            FlatRes::One(t) => flat(&t),
            FlatRes::Retptr => {
                p.push(I32);
                vec![]
            }
        };
        let module = versioned_iface(&dep.package, &iface);
        // Declare the import once per mangled name; a method shared across ops
        // (none today) would otherwise be added twice.
        if !em
            .import_fn
            .contains_key(&(module.clone(), sig.name.clone()))
        {
            add_import(&mut em, &module, &sig.name, p, r);
        }
    }

    // ---- functor resource-intrinsic imports (one triple per instantiation)
    //
    // For every `Set` functor instantiation the encoder synthesizes three
    // resource intrinsics — `[resource-new/rep/drop]set` — under the module
    // string `[export]<versioned specialized iface>` (summary 01 §2). They MUST
    // be declared here, in the imports-first index block, so the function index
    // space stays imports-first; their indices are captured for `emit_set_resource`
    // (which the constructor calls `resource.new` through). The intrinsics only
    // exist because the synthesized world *exports* the resource interface.
    let mut functor_intrinsics: Vec<(u32, u32, u32)> = Vec::new();
    for inst in &info.functors {
        let module = format!("[export]{}", versioned_iface(&info.package, &inst.iface));
        add_import(&mut em, &module, "[resource-new]set", vec![I32], vec![I32]);
        add_import(&mut em, &module, "[resource-rep]set", vec![I32], vec![I32]);
        add_import(&mut em, &module, "[resource-drop]set", vec![I32], vec![]);
        let new_i = em.import_idx(&module, "[resource-new]set");
        let rep_i = em.import_idx(&module, "[resource-rep]set");
        let drop_i = em.import_idx(&module, "[resource-drop]set");
        functor_intrinsics.push((new_i, rep_i, drop_i));
    }

    // ---- user-resource resource-intrinsic imports (one triple per resource)
    //
    // `[resource-new/rep/drop]<name>` under `[export]<versioned export iface>`,
    // exactly as for the functor `set`. Only exported resources are supported by
    // the backend; an unexported `DefResource` has no boundary interface to hang
    // the intrinsics on.
    let mut user_res_intrinsics: HashMap<String, (u32, u32, u32)> = HashMap::new();
    for rf in &user_res_forms {
        let Some(decl) = info.resources.iter().find(|r| r.name == rf.name) else {
            continue;
        };
        let Some(iface_path) = &decl.iface else {
            return Err(format!(
                "resource `{}` is declared but not exported; the wasm backend only \
                 supports exported resources yet",
                rf.name
            ));
        };
        let iface = if is_external_iface(iface_path) {
            external_versioned_in(iface_path, deps)
        } else {
            versioned_iface(&info.package, iface_path)
        };
        let module = format!("[export]{iface}");
        let nnew = format!("[resource-new]{}", rf.name);
        let nrep = format!("[resource-rep]{}", rf.name);
        let ndrop = format!("[resource-drop]{}", rf.name);
        add_import(&mut em, &module, &nnew, vec![I32], vec![I32]);
        add_import(&mut em, &module, &nrep, vec![I32], vec![I32]);
        add_import(&mut em, &module, &ndrop, vec![I32], vec![]);
        let ni = em.import_idx(&module, &nnew);
        let ri = em.import_idx(&module, &nrep);
        let di = em.import_idx(&module, &ndrop);
        user_res_intrinsics.insert(rf.name.clone(), (ni, ri, di));
    }

    // ---- assign helper indices
    let mut next = n_imports;
    let mut take = || {
        let i = next;
        next += 1;
        i
    };
    em.h.alloc = take();
    em.h.realloc = take();
    em.h.box_int = take();
    em.h.box_bool = take();
    em.h.box_dec = take();
    em.h.box_str = take();
    em.h.truthy = take();
    em.h.unbox_int = take();
    em.h.unbox_char = take();
    em.h.unbox_dec = take();
    em.h.eq_raw = take();
    em.h.len_raw = take();
    em.h.head_h = take();
    em.h.tail_h = take();
    em.h.strcat2 = take();
    em.h.case_h = take();
    em.h.to_str = take();
    em.h.rec_get = take();
    em.h.as_f64 = take();
    em.h.arith_raw = take();
    em.h.cmp_raw = take();
    em.h.neg_raw = take();
    em.h.arith_int = take();
    em.h.cmp_f64 = take();
    em.h.persist_alloc = take();
    em.h.persist = take();

    // ---- reserve the functor `set` resource core-func indices (step 04)
    //
    // The five resource funcs (ctor/add/contains/size/dtor) per instantiation are
    // EMITTED just after `emit_helpers` below, so in `em.bodies` they sit directly
    // after the helper bodies and before the internal/export bodies. Reserve their
    // indices here, in that exact position in the `take()` sequence, so the indices
    // recorded in `em.functor_fns` match where `emit_set_resource` will self-index
    // them — and so the internal/overload/export `take()`s that follow are shifted
    // past the resource slots. `dep_call` reads `em.functor_fns` to route an
    // `alias/op` call while the internal/export bodies (which contain those calls)
    // are still being lowered, i.e. before the resource bodies exist.
    for (inst, &(new_i, rep_i, drop_i)) in info.functors.iter().zip(&functor_intrinsics) {
        let ctor = take();
        let add = take();
        let contains = take();
        let size = take();
        let dtor = take();
        em.functor_fns.insert(
            inst.alias.clone(),
            ResourceFns {
                ctor,
                add,
                contains,
                size,
                dtor,
                new_import: new_i,
                rep_import: rep_i,
                drop_import: drop_i,
            },
        );
    }

    // ---- reserve user-resource member fn indices (4.5)
    //
    // Reserve indices for each resource's constructor, methods, statics, and dtor
    // in the same take() position their bodies are emitted (right after the functor
    // `set` bodies), so indices line up. The constructor is registered under the
    // bare resource name and each method/static under `name/op`, so ordinary call
    // resolution (the `Sym`/`Qsym` arms) finds them; routing data goes in `user_res`.
    for rf in &user_res_forms {
        let (_ni, _ri, _di) = user_res_intrinsics[&rf.name];
        if let Some((pid, _)) = rf.ctor {
            let names = param_names(arena, pid)?;
            let n = names.len();
            let idx = take();
            em.funcs.insert(
                rf.name.clone(),
                (idx, names, FnSig { params: vec![Repr::Boxed; n], result: Repr::Boxed }),
            );
        }
        let mut methods = std::collections::HashSet::new();
        for (key, pid, _) in &rf.methods {
            let names = param_names(arena, *pid)?;
            let n = names.len();
            let idx = take();
            em.funcs.insert(
                format!("{}/{}", rf.name, key),
                (idx, names, FnSig { params: vec![Repr::Boxed; n], result: Repr::Boxed }),
            );
            methods.insert(key.clone());
        }
        let mut statics = std::collections::HashSet::new();
        for (key, pid, _) in &rf.statics {
            let names = param_names(arena, *pid)?;
            let n = names.len();
            let idx = take();
            em.funcs.insert(
                format!("{}/{}", rf.name, key),
                (idx, names, FnSig { params: vec![Repr::Boxed; n], result: Repr::Boxed }),
            );
            statics.insert(key.clone());
        }
        let dtor = take();
        em.user_res.insert(rf.name.clone(), UserRes { methods, statics, dtor });
    }

    // An *exported overload set* (≥2 same-named `Def Fn`s, or a curated-op name)
    // is lowered by `wit::collect` to one mangled WIT export per member
    // (`eq-point`, `eq-string`, …), recorded in `info.overload_bodies` as
    // mangled-name -> (params, body). The underlying `Def`s share one original
    // name (`eq`) which collapses last-wins in `info.defs`, so the export
    // wrappers — which look bodies up by the *mangled* name — would otherwise
    // find nothing (`export `eq-point` has no Def Fn`). Register and emit one
    // internal function per mangled member instead, keyed on identity. Skip the
    // original collapsed name in the normal pass below so we don't emit a bogus,
    // unreferenced `eq` (and avoid any clash). Keep `info.fn_defs`/overload sets
    // intact — internal-call resolution and type-checking land in a later step.
    let mut overload_order: Vec<String> = info.overload_bodies.keys().cloned().collect();
    overload_order.sort(); // deterministic index assignment
    // Original def names whose every member was consumed by overload mangling.
    let overloaded_origins: std::collections::HashSet<&String> = info
        .fn_defs
        .iter()
        .filter(|(_, members)| {
            members
                .iter()
                .all(|m| info.overload_bodies.values().any(|ob| ob == m))
        })
        .map(|(name, _)| name)
        .collect();

    // ---- assign internal function indices (file order)
    let mut internal_order: Vec<String> = Vec::new();
    for &root in roots {
        if let Node::Tup(items) = arena.node(root)
            && items.len() >= 2
            && matches!(arena.node(items[0]), Node::Sym(s) if s == "def-MACRO")
            && let Node::Sym(name) = arena.node(items[1])
            && info.defs.contains_key(name)
            && !overloaded_origins.contains(name)
            && !internal_order.contains(name)
        {
            internal_order.push(name.clone());
        }
    }
    for (i, (name, _)) in info.value_defs.iter().enumerate() {
        em.value_globals.insert(name.clone(), 1 + i as u32); // global 0 = heap ptr
    }
    for name in &internal_order {
        let (params_id, body) = info.defs[name];
        let params = param_names(arena, params_id)?;
        let sig = em.def_sig(params_id, body);
        em.funcs.insert(name.clone(), (take(), params, sig));
    }
    // Mangled overload members get their own internal-function indices.
    for mangled in &overload_order {
        let (params_id, body) = info.overload_bodies[mangled];
        let params = param_names(arena, params_id)?;
        let sig = em.def_sig(params_id, body);
        em.funcs.insert(mangled.clone(), (take(), params, sig));
    }

    // ---- reserve lifted-lambda indices (5.8 Fn-literal capture devirt)
    //
    // Scan every internal def body for `Fn`-literal Let inits with a concrete
    // arrow type and reserve one core-function index per hit, HERE — right after
    // the overload block and before the export wrappers (which `take()` their
    // own indices later). The lifted bodies are pushed into `em.bodies` between
    // the overload bodies and the export wrappers, in this same reservation
    // order, so positions match indices (asserted at the push site).
    for name in &internal_order {
        let (_, body) = info.defs[name];
        scan_let_lambdas(arena, body, &em.node_types, &mut em.lambda_order);
    }
    for &node in &em.lambda_order.clone() {
        let idx = take();
        em.lambda_reserved.insert(node, idx);
    }

    // ---- helper bodies (order must match index assignment above)
    emit_helpers(&mut em)?;

    // ---- functor `set` resource bodies (step 02 bodies; emitted here in step 04)
    //
    // Emitted right after the helpers and BEFORE the internal/export bodies, so
    // each resource's five `em.bodies` positions line up with the indices reserved
    // in `em.functor_fns` above. `dep_call` routes `alias/op` calls to those
    // reserved indices; the canonical export NAMES are registered later (alongside
    // the export wrappers). The element type is resolved through the boundary
    // `type_env`; `flat_checked` inside `emit_set_resource` rejects any element the
    // backend can't flatten with an honest error.
    for (inst, &(new_i, rep_i, drop_i)) in info.functors.iter().zip(&functor_intrinsics) {
        let elem = wit_ty(&inst.elem, &em.type_env).map_err(|e| {
            format!(
                "functor `set` over `{}` (alias `{}`): {e}",
                inst.elem, inst.alias
            )
        })?;
        let fns = emit_set_resource(&mut em, inst, &elem, new_i, rep_i, drop_i)?;
        debug_assert_eq!(
            em.functor_fns.get(&inst.alias).copied(),
            Some(fns),
            "reserved functor `set` indices must match the emitted body positions"
        );
    }

    // ---- user-resource member bodies (4.5)
    //
    // Emitted right after the functor `set` bodies so positions match the indices
    // reserved above (constructor, methods, statics, dtor per resource, in that
    // order). A resource *value* is carried guest-internally as its REP (the cell
    // box the `New` body returns) — the same thing the interpreter binds `self`
    // to and that `cell-get`/`cell-set` operate on. So the constructor, methods,
    // and statics are ORDINARY internal bodies over reps; the own/borrow handle
    // conversions (`resource.new`/`resource.rep`) happen only at the boundary.
    // The dtor runs `Drop` (rep = param 0) or no-ops.
    for rf in &user_res_forms {
        let mut member_bodies: Vec<(String, NodeId)> = Vec::new();
        if let Some((_pid, body)) = rf.ctor {
            member_bodies.push((rf.name.clone(), body));
        }
        for (key, _pid, body) in rf.methods.iter().chain(rf.statics.iter()) {
            member_bodies.push((format!("{}/{}", rf.name, key), *body));
        }
        for (fname, body) in member_bodies {
            let (_, params, sig) = em.funcs[&fname].clone();
            let mut fx = FnCtx::new(params.len() as u32);
            let mut scope = HashMap::new();
            for (i, pn) in params.iter().enumerate() {
                scope.insert(pn.clone(), Binding::new(i as u32, sig.params[i] ));
            }
            fx.scopes.push(scope);
            em.expr_repr(&mut fx, body, sig.result, true)
                .map_err(|e| format!("in resource member `{fname}`: {e}"))?;
            let t = em.ty_idx(sig.param_vts(), vec![repr_vt(sig.result)]);
            em.bodies.push((t, fx.finish()));
        }
        {
            let mut fx = FnCtx::new(1);
            if let Some((pid, body)) = rf.drop_member {
                let mut scope = HashMap::new();
                if let Some(pn) = param_names(arena, pid)?.first() {
                    scope.insert(pn.clone(), Binding::new(0, Repr::Boxed ));
                }
                fx.scopes.push(scope);
                em.expr_repr(&mut fx, body, Repr::Boxed, false)
                    .map_err(|e| format!("in resource `{}` Drop: {e}", rf.name))?;
                fx.op(I::Drop); // discard the unit result
            }
            let t = em.ty_idx(vec![ValType::I32], vec![]);
            em.bodies.push((t, fx.finish()));
        }
    }

    // ---- internal function bodies (typed per their repr signature — 5.2)
    for name in &internal_order {
        let (_, body) = info.defs[name];
        let (_, params, sig) = em.funcs[name].clone();
        let n = params.len();
        let mut fx = FnCtx::new(n as u32);
        let mut scope = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            scope.insert(
                p.clone(),
                Binding::new(i as u32, sig.params[i]),
            );
        }
        fx.scopes.push(scope);
        em.expr_repr(&mut fx, body, sig.result, true)
            .map_err(|e| format!("in `{name}`: {e}"))?;
        let t = em.ty_idx(sig.param_vts(), vec![repr_vt(sig.result)]);
        em.bodies.push((t, fx.finish()));
    }
    // ---- mangled overload member bodies (same paired order as their indices)
    for mangled in &overload_order {
        let (_, body) = info.overload_bodies[mangled];
        let (_, params, sig) = em.funcs[mangled].clone();
        let n = params.len();
        let mut fx = FnCtx::new(n as u32);
        let mut scope = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            scope.insert(
                p.clone(),
                Binding::new(i as u32, sig.params[i]),
            );
        }
        fx.scopes.push(scope);
        em.expr_repr(&mut fx, body, sig.result, true)
            .map_err(|e| format!("in `{mangled}`: {e}"))?;
        let t = em.ty_idx(sig.param_vts(), vec![repr_vt(sig.result)]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- lifted lambda bodies (5.8 Fn-literal capture devirtualization)
    //
    // Pushed here — after the overload bodies, before the export wrappers — so
    // each body's position matches the index reserved for it right after the
    // overload block (asserted below). Every reserved node was compiled during
    // its enclosing internal def body (`compile_known_lambda` stashed it), so
    // the stash is complete.
    for node in em.lambda_order.clone() {
        let (t, f) = em
            .lambda_stash
            .remove(&node)
            .ok_or_else(|| format!("internal: lifted lambda body for node {node} missing"))?;
        debug_assert_eq!(
            n_imports + em.bodies.len() as u32,
            em.lambda_reserved[&node],
            "lifted lambda body position must match its reserved index"
        );
        em.bodies.push((t, f));
    }

    // ---- export wrappers
    let mut exports: Vec<(String, u32)> = Vec::new(); // (export name, fn idx)
    // Each export's (full export name, flat results) — a canonical post-return
    // companion is emitted per entry after this loop (5.1 arena-per-call).
    let mut post_returns: Vec<(String, Vec<ValType>)> = Vec::new();
    // Classify a boundary type text as a user-resource handle (4.5): a resource
    // value crosses the boundary as an own handle (`counter`) or a borrow
    // (`borrow<counter>`), but is carried guest-internally as its rep. `own`
    // params/results convert with `resource.rep`/`resource.new`; a `borrow` param
    // already arrives AS the rep (the canonical ABI hands a borrow of a
    // self-owned resource the rep directly). The functor `set` (handle carriage)
    // is deliberately excluded — it is not in `user_res_intrinsics`.
    let classify_res = |t: &str| -> Option<(String, bool)> {
        if let Some(inner) = t.strip_prefix("borrow<").and_then(|x| x.strip_suffix('>'))
            && user_res_intrinsics.contains_key(inner)
        {
            return Some((inner.to_string(), true));
        }
        let bare = t
            .strip_prefix("own<")
            .and_then(|x| x.strip_suffix('>'))
            .unwrap_or(t);
        if user_res_intrinsics.contains_key(bare) {
            return Some((bare.to_string(), false));
        }
        None
    };
    for sig in &info.exports {
        let (fidx, _, fsig) = em
            .funcs
            .get(&sig.name)
            .cloned()
            .ok_or(format!("export `{}` has no Def Fn", sig.name))?;
        let mut fparams = Vec::new();
        let mut lifted: Vec<(WitTy, u32, Option<(String, bool)>)> = Vec::new();
        for (_, t) in &sig.params {
            let ty = wit_ty(t, &em.type_env)?;
            let res = classify_res(t);
            lifted.push((ty.clone(), fparams.len() as u32, res));
            fparams.extend_from_slice(&flat_checked(&ty)?);
        }
        if fparams.len() > 16 {
            return Err(format!(
                "`{}` flattens to {} parameters; spilling >16 params to memory \
                 is not supported by the wasm backend yet",
                sig.name,
                fparams.len()
            ));
        }
        let mut fx = FnCtx::new(fparams.len() as u32);
        // 5.2 export fast path: a typed def param whose WIT type is the same
        // scalar kind receives the flat ABI value DIRECTLY (extend/promote),
        // skipping the box entirely; everything else lifts to a box as
        // before (plus one unbox if the def param is typed anyway).
        let typed_ok = fsig.params.len() == lifted.len();
        for (i, (ty, base, res)) in lifted.iter().enumerate() {
            if let Some((rname, is_borrow)) = res {
                // own: `resource.rep(handle) -> rep`; borrow: the flat i32 already
                // IS the rep box pointer.
                fx.op(I::LocalGet(*base));
                if !is_borrow {
                    let (_n, rep_i, _d) = user_res_intrinsics[rname];
                    fx.op(I::Call(rep_i));
                }
                continue;
            }
            let slot = if typed_ok {
                fsig.params[i]
            } else {
                Repr::Boxed
            };
            match (slot, wit_scalar(ty)) {
                (Repr::Scalar(k), Some(wk)) if k == wk => {
                    fx.op(I::LocalGet(*base));
                    match ty {
                        WitTy::Bool => {
                            // normalize to 0/1, like the interpreter's Bool
                            fx.op(I::I32Const(0));
                            fx.op(I::I32Ne);
                        }
                        WitTy::IntS(_) => fx.op(I::I64ExtendI32S),
                        WitTy::IntU(_) | WitTy::Char => fx.op(I::I64ExtendI32U),
                        WitTy::S64 | WitTy::F64 => {}
                        WitTy::F32 => fx.op(I::F64PromoteF32),
                        _ => unreachable!("wit_scalar admitted a non-scalar"),
                    }
                }
                (Repr::Scalar(k), _) => {
                    em.lift_flat(&mut fx, ty, *base)?;
                    em.unbox_scalar(&mut fx, k);
                }
                (Repr::Boxed, _) => em.lift_flat(&mut fx, ty, *base)?,
                (Repr::Mem(_), _) => {
                    return Err("internal: Mem def params are not emitted yet (5.3)".into());
                }
            }
        }
        fx.op(I::Call(fidx));
        // the call produced the def's result repr; the paths below expect
        // the value they were written for (flat scalar or box)
        let res_kind = if typed_ok { fsig.result } else { Repr::Boxed };
        let fresults = match flat_result(sig, &em.type_env)? {
            FlatRes::None => {
                fx.op(I::Drop);
                vec![]
            }
            FlatRes::One(t) if classify_res(sig.result.as_deref().unwrap_or("")).is_some() => {
                // A user-resource own result: the def returned the rep; mint the
                // own handle to hand out across the boundary.
                let (rname, _) = classify_res(sig.result.as_deref().unwrap()).unwrap();
                let (new_i, _r, _d) = user_res_intrinsics[&rname];
                if let Repr::Scalar(k) = res_kind {
                    em.box_scalar(&mut fx, k);
                }
                fx.op(I::Call(new_i));
                flat(&t)
            }
            FlatRes::One(t) => {
                match res_kind {
                    // typed result, same scalar kind as the WIT type: convert
                    // directly to the flat form — no box was ever built
                    Repr::Scalar(k) if wit_scalar(&t) == Some(k) => match t {
                        WitTy::Bool | WitTy::S64 | WitTy::F64 => {}
                        WitTy::IntS(_) | WitTy::IntU(_) | WitTy::Char => fx.op(I::I32WrapI64),
                        WitTy::F32 => fx.op(I::F32DemoteF64),
                        _ => unreachable!("wit_scalar admitted a non-scalar"),
                    },
                    // typed result but a non-scalar/mismatched WIT type:
                    // box at the seam and lower as before
                    Repr::Scalar(k) => {
                        em.box_scalar(&mut fx, k);
                        em.lower(&mut fx, &t)?;
                    }
                    Repr::Boxed => em.lower(&mut fx, &t)?,
                    Repr::Mem(mt) => {
                        let l = fx.local(I32);
                        fx.op(I::LocalSet(l));
                        let mty = em.mem_tys[mt as usize].clone();
                        em.load_from_mem(&mut fx, &mty, l, 0)?;
                        em.lower(&mut fx, &t)?;
                    }
                }
                flat(&t)
            }
            FlatRes::Retptr => {
                let ty = wit_ty(sig.result.as_deref().unwrap(), &em.type_env)?;
                // 5.3 fast path: the def's result already lives in canonical
                // layout of exactly the boundary type — the pointer on the
                // stack IS the callee-owned result area. No box, no copy.
                if matches!(res_kind, Repr::Mem(mt) if em.mem_tys[mt as usize] == ty) {
                    let t = em.ty_idx(fparams, vec![I32]);
                    em.bodies.push((t, fx.finish()));
                    let own_iface = if is_external_iface(&sig.iface) {
                        external_versioned_in(&sig.iface, deps)
                    } else {
                        versioned_iface(&info.package, &sig.iface)
                    };
                    let export_name = format!("{own_iface}#{}", sig.name);
                    post_returns.push((export_name.clone(), vec![I32]));
                    exports.push((export_name, take()));
                    continue;
                }
                // compound results are boxed; a (mismatched) typed scalar or
                // canonical result reboxes at the seam first
                match res_kind {
                    Repr::Boxed => {}
                    Repr::Scalar(k) => em.box_scalar(&mut fx, k),
                    Repr::Mem(mt) => {
                        let l = fx.local(I32);
                        fx.op(I::LocalSet(l));
                        let mty = em.mem_tys[mt as usize].clone();
                        em.load_from_mem(&mut fx, &mty, l, 0)?;
                    }
                }
                let area = fx.local(I32);
                if matches!(
                    ty,
                    WitTy::Record(_)
                        | WitTy::Tuple(_)
                        | WitTy::Option(_)
                        | WitTy::Result(..)
                        | WitTy::Variant(_)
                ) {
                    // store the value's canonical layout into a callee-owned area
                    let rbox = fx.local(I32);
                    fx.op(I::LocalSet(rbox));
                    fx.op(I::I32Const(size_of(&ty) as i32));
                    fx.op(I::Call(em.h.alloc));
                    fx.op(I::LocalSet(area));
                    em.store_to_mem(&mut fx, &ty, rbox, area, 0)?;
                    fx.op(I::LocalGet(area));
                } else {
                    // string/list: lower to (ptr, len) parked in an 8-byte area
                    em.lower(&mut fx, &ty)?;
                    let lp = fx.local(I32);
                    let ll = fx.local(I32);
                    fx.op(I::LocalSet(ll));
                    fx.op(I::LocalSet(lp));
                    fx.op(I::I32Const(8));
                    fx.op(I::Call(em.h.alloc));
                    fx.op(I::LocalTee(area));
                    fx.op(I::LocalGet(lp));
                    fx.op(I::I32Store(ma(0, 2)));
                    fx.op(I::LocalGet(area));
                    fx.op(I::LocalGet(ll));
                    fx.op(I::I32Store(ma(4, 2)));
                    fx.op(I::LocalGet(area));
                }
                vec![I32]
            }
        };
        let t = em.ty_idx(fparams, fresults.clone());
        em.bodies.push((t, fx.finish()));
        // An external interface (wasi:http/incoming-handler, wasi:cli/run) is
        // exported under its own versioned name — at the version of its resolved
        // `wit/deps` package on the generic path, or the vendored WASI version
        // for the magic path; a local one lands in this package.
        let own_iface = if is_external_iface(&sig.iface) {
            external_versioned_in(&sig.iface, deps)
        } else {
            versioned_iface(&info.package, &sig.iface)
        };
        let export_name = format!("{own_iface}#{}", sig.name);
        post_returns.push((export_name.clone(), fresults));
        exports.push((export_name, take()));
    }

    // ---- per-call arena reset via canonical post-return (5.1)
    //
    // The memory story decided on 5.1 (arena-per-call, headerless): every
    // export gets a `cabi_post_<export-name>` companion, which wit-component
    // wires as the export's post-return function and the host runtime invokes
    // once the caller has finished reading the results. At that point nothing
    // allocated during the call is live — the lowered arguments, the call's
    // temporaries, and the result area are all dead — so the bump pointer
    // resets to the arena floor (the heap base) and the lazily-cached
    // value-def globals clear to recompute on the next call (their cached
    // boxes died with the arena; value defs are pure in the backend, so
    // recomputation is semantics-neutral). Interned statics live below the
    // heap base and are untouched.
    //
    // Components with functor instantiations OR user-declared resources (4.5)
    // used to OPT OUT of the reset and keep never-free behaviour, because a
    // resource's rep — the `set` cell and its stored list, or a user `counter`'s
    // `New` cell — must survive across export calls. As of the 5.1 evacuation
    // that state now lives in the PERSISTENT region below the arena floor
    // (`persist_alloc` + the `persist` write barrier, wired into cell-new/set and
    // the functor `set` ctor/add), which the reset does not touch — so every
    // component, resource-bearing or not, resets its arena at post-return.
    //
    // Global indices: 0 = arena bump ptr, 1..=n value defs, 1+n gensym counter,
    // 2+n = the arena floor (heap_base + persistent reserve for resource
    // components, heap_base otherwise), 3+n = the persistent bump ptr.
    let arena_floor_g = 2 + info.value_defs.len() as u32;
    for (name, fresults) in post_returns {
        let mut fx = FnCtx::new(fresults.len() as u32);
        fx.op(I::GlobalGet(arena_floor_g));
        fx.op(I::GlobalSet(0));
        for i in 0..info.value_defs.len() {
            fx.op(I::I32Const(0));
            fx.op(I::GlobalSet(1 + i as u32));
        }
        let t = em.ty_idx(fresults, vec![]);
        em.bodies.push((t, fx.finish()));
        exports.push((format!("cabi_post_{name}"), take()));
    }

    // ---- user-resource boundary exports (4.5)
    //
    // Emit the canonical resource-ABI boundary functions per exported resource:
    // `[constructor]`/`[method]`/`[static]` wrappers that lift the flat params to
    // boxes, call the internal member fn, and lower the boxed result back to the
    // flat ABI; the `[dtor]` body was emitted above and is exported directly. A
    // constructor returns the own handle (unbox the internal fn's boxed handle
    // value); a method receives the rep as core param 0 (already a box pointer).
    for rf in &user_res_forms {
        let Some(decl) = info.resources.iter().find(|r| r.name == rf.name) else {
            continue;
        };
        let Some(iface_path) = &decl.iface else {
            continue;
        };
        let iface = if is_external_iface(iface_path) {
            external_versioned_in(iface_path, deps)
        } else {
            versioned_iface(&info.package, iface_path)
        };
        let (new_i, _rep_i, _drop_i) = user_res_intrinsics[&rf.name];
        if let Some(ctor_params) = &decl.constructor {
            let ctor_idx = em.funcs[&rf.name].0;
            let mut fparams = Vec::new();
            let mut lifted: Vec<(WitTy, u32)> = Vec::new();
            for (_, t) in ctor_params {
                let ty = wit_ty(t, &em.type_env)?;
                lifted.push((ty.clone(), fparams.len() as u32));
                fparams.extend_from_slice(&flat_checked(&ty)?);
            }
            let mut fx = FnCtx::new(fparams.len() as u32);
            for (ty, base) in &lifted {
                em.lift_flat(&mut fx, ty, *base)?;
            }
            fx.op(I::Call(ctor_idx)); // -> rep pointer (the New cell)
            fx.op(I::Call(new_i)); // mint an own handle from the rep
            let t = em.ty_idx(fparams, vec![I32]);
            em.bodies.push((t, fx.finish()));
            exports.push((format!("{iface}#[constructor]{}", rf.name), take()));
        }
        for m in &decl.methods {
            let midx = em.funcs[&format!("{}/{}", rf.name, m.name)].0;
            let mut fparams = vec![I32]; // rep (self)
            let mut lifted: Vec<(WitTy, u32)> = Vec::new();
            for (_, t) in &m.params {
                let ty = wit_ty(t, &em.type_env)?;
                lifted.push((ty.clone(), fparams.len() as u32));
                fparams.extend_from_slice(&flat_checked(&ty)?);
            }
            let mut fx = FnCtx::new(fparams.len() as u32);
            fx.op(I::LocalGet(0)); // rep as the boxed `self` argument
            for (ty, base) in &lifted {
                em.lift_flat(&mut fx, ty, *base)?;
            }
            fx.op(I::Call(midx));
            let fresults = match flat_result(m, &em.type_env)? {
                FlatRes::None => {
                    fx.op(I::Drop);
                    vec![]
                }
                FlatRes::One(WitTy::Handle) => {
                    fx.op(I::Call(new_i)); // rep -> own handle
                    vec![I32]
                }
                FlatRes::One(t) => {
                    em.lower(&mut fx, &t)?;
                    flat(&t)
                }
                FlatRes::Retptr => {
                    return Err(format!(
                        "resource `{}` method `{}` returns a memory-spilled type; \
                         not supported by the wasm backend yet",
                        rf.name, m.name
                    ));
                }
            };
            let t = em.ty_idx(fparams, fresults);
            em.bodies.push((t, fx.finish()));
            exports.push((format!("{iface}#[method]{}.{}", rf.name, m.name), take()));
        }
        for st in &decl.statics {
            let sidx = em.funcs[&format!("{}/{}", rf.name, st.name)].0;
            let mut fparams = Vec::new();
            let mut lifted: Vec<(WitTy, u32)> = Vec::new();
            for (_, t) in &st.params {
                let ty = wit_ty(t, &em.type_env)?;
                lifted.push((ty.clone(), fparams.len() as u32));
                fparams.extend_from_slice(&flat_checked(&ty)?);
            }
            let mut fx = FnCtx::new(fparams.len() as u32);
            for (ty, base) in &lifted {
                em.lift_flat(&mut fx, ty, *base)?;
            }
            fx.op(I::Call(sidx));
            let fresults = match flat_result(st, &em.type_env)? {
                FlatRes::None => {
                    fx.op(I::Drop);
                    vec![]
                }
                FlatRes::One(WitTy::Handle) => {
                    fx.op(I::Call(new_i)); // rep -> own handle
                    vec![I32]
                }
                FlatRes::One(t) => {
                    em.lower(&mut fx, &t)?;
                    flat(&t)
                }
                FlatRes::Retptr => {
                    return Err(format!(
                        "resource `{}` static `{}` returns a memory-spilled type; \
                         not supported by the wasm backend yet",
                        rf.name, st.name
                    ));
                }
            };
            let t = em.ty_idx(fparams, fresults);
            em.bodies.push((t, fx.finish()));
            exports.push((format!("{iface}#[static]{}.{}", rf.name, st.name), take()));
        }
        exports.push((
            format!("{iface}#[dtor]{}", rf.name),
            em.user_res[&rf.name].dtor,
        ));
    }

    let _ = take; // `next`/`take` are done; the resource bodies were emitted above.

    // ---- functor `set` resource EXPORTS (canonical names) — one per instantiation
    //
    // The five core funcs were already emitted right after the helpers (so their
    // indices match `em.functor_fns`). Here we only register their canonical
    // resource-ABI export names, prefixed by the versioned specialized interface
    // (`demo:geo/point-set@0.1.0`); the index for each comes from the reserved
    // `em.functor_fns`. `ExportSection` later writes these verbatim, same as any
    // ordinary export.
    for inst in &info.functors {
        let fns = em.functor_fns[&inst.alias];
        let iface = versioned_iface(&info.package, &inst.iface);
        exports.push((format!("{iface}#[constructor]set"), fns.ctor));
        exports.push((format!("{iface}#[method]set.add"), fns.add));
        exports.push((format!("{iface}#[method]set.contains"), fns.contains));
        exports.push((format!("{iface}#[method]set.size"), fns.size));
        exports.push((format!("{iface}#[dtor]set"), fns.dtor));
    }

    // ---- assemble
    let heap_base = {
        em.align8();
        DATA_BASE + em.data.len() as u32
    };
    // 5.1: resource/functor components reserve a persistent region [heap_base,
    // arena_floor) for resource state that outlives the per-call arena reset;
    // the arena bump pointer starts at (and resets to) `arena_floor`. Other
    // components reserve nothing, so arena_floor == heap_base as before.
    let has_persist = !info.functors.is_empty() || !info.resources.is_empty();
    let persist_reserve = if has_persist { PERSIST_RESERVE } else { 0 };
    let arena_floor = heap_base + persist_reserve;
    let pages = (arena_floor as u64 >> 16) + 1;

    let mut module = Module::new();
    let mut ts = TypeSection::new();
    for (p, r) in &em.types {
        ts.ty().function(p.iter().copied(), r.iter().copied());
    }
    module.section(&ts);

    let mut is = ImportSection::new();
    for (m, f, t) in &em.imports {
        is.import(m, f, EntityType::Function(*t));
    }
    module.section(&is);

    // closure/wrapper functions live after every directly-indexed function;
    // table slot k = function index closure_base + k
    let closure_base = n_imports + em.bodies.len() as u32;

    let mut fs = FunctionSection::new();
    for (t, _) in &em.bodies {
        fs.function(*t);
    }
    for (t, _) in &em.closure_bodies {
        fs.function(*t);
    }
    module.section(&fs);

    if !em.closure_bodies.is_empty() {
        let mut tbl = TableSection::new();
        tbl.table(TableType {
            element_type: RefType::FUNCREF,
            minimum: em.closure_bodies.len() as u64,
            maximum: Some(em.closure_bodies.len() as u64),
            table64: false,
            shared: false,
        });
        module.section(&tbl);
    }

    let mut ms = MemorySection::new();
    ms.memory(MemoryType {
        minimum: pages,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&ms);

    let mut gs = GlobalSection::new();
    // global 0: the arena bump pointer. Starts at the arena floor (above the
    // persistent reserve for resource components; == heap_base otherwise).
    gs.global(
        GlobalType {
            val_type: I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(arena_floor as i32),
    );
    for _ in &info.value_defs {
        gs.global(
            GlobalType {
                val_type: I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(0),
        );
    }
    // The `gensym` counter (always present): an i64 incremented once per
    // `gensym` call, so fresh symbols are unique and deterministic across every
    // expansion in one component instance. Index = 1 + value_defs.len() (global
    // 0 is the heap pointer, then one i32 per value def).
    gs.global(
        GlobalType {
            val_type: ValType::I64,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i64_const(0),
    );
    // The arena floor (5.1 arena-per-call): the base the per-export post-return
    // bodies reset the arena bump pointer to. For a resource/functor component
    // it sits above the persistent reserve; otherwise it is the heap base.
    // Index = 2 + value_defs.len() (see the post-return emission above).
    gs.global(
        GlobalType {
            val_type: I32,
            mutable: false,
            shared: false,
        },
        &ConstExpr::i32_const(arena_floor as i32),
    );
    // 5.1 persistent bump pointer (index 3 + value_defs.len()). Grows up from
    // heap_base within the reserve; never reset. Left at heap_base and unused
    // by non-resource components.
    gs.global(
        GlobalType {
            val_type: I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(heap_base as i32),
    );
    module.section(&gs);

    let mut es = ExportSection::new();
    es.export("memory", ExportKind::Memory, 0);
    es.export("cabi_realloc", ExportKind::Func, em.h.realloc);
    for (name, idx) in &exports {
        es.export(name, ExportKind::Func, *idx);
    }
    module.section(&es);

    if !em.closure_bodies.is_empty() {
        let idxs: Vec<u32> = (0..em.closure_bodies.len() as u32)
            .map(|k| closure_base + k)
            .collect();
        let mut els = ElementSection::new();
        els.active(
            Some(0),
            &ConstExpr::i32_const(0),
            Elements::Functions(idxs.into()),
        );
        module.section(&els);
    }

    let mut cs = CodeSection::new();
    for (_, f) in &em.bodies {
        cs.function(f);
    }
    for (_, f) in &em.closure_bodies {
        cs.function(f);
    }
    module.section(&cs);

    let mut ds = DataSection::new();
    ds.active(
        0,
        &ConstExpr::i32_const(DATA_BASE as i32),
        em.data.iter().copied(),
    );
    module.section(&ds);

    Ok(module.finish())
}
