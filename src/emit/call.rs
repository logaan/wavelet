//! `Emitter` methods for calls and binding forms: `Fn` literals, closure and
//! internal calls, argument binding, cross-component dep calls, and `Let`.

use super::*;

impl<'a> Emitter<'a> {
    /// `Fn {params} body` as an expression: compile the body to a uniform
    /// `(env, payload) -> box` table function capturing every visible local,
    /// and allocate a closure box `[TAG_FN, slot, k, captures…]` at the site.
    pub(crate) fn fn_form(&mut self, fx: &mut FnCtx, args: &[NodeId]) -> Result<(), String> {
        let [params_id, body] = *args else {
            return Err("malformed Fn".into());
        };
        let params = param_names(self.arena, params_id)?;

        // captures: every visible local by name (later scopes shadow earlier),
        // sorted so the layout is deterministic
        let mut cap_map: HashMap<String, Binding> = HashMap::new();
        for scope in &fx.scopes {
            for (k, &v) in scope {
                cap_map.insert(k.clone(), v);
            }
        }
        let mut caps: Vec<(String, Binding)> = cap_map.into_iter().collect();
        caps.sort_by(|a, b| a.0.cmp(&b.0));

        let mut cf = FnCtx::new(2);
        let mut scope = HashMap::new();
        for (j, (cname, _)) in caps.iter().enumerate() {
            let l = cf.local(ValType::I32);
            cf.op(I::LocalGet(0));
            cf.op(I::I32Load(ma(12 + 4 * j as u64, 2)));
            cf.op(I::LocalSet(l));
            scope.insert(cname.clone(), Binding::boxed(l));
        }
        match params.len() {
            0 => {}
            1 => {
                scope.insert(params[0].clone(), Binding::boxed(1));
            }
            n => {
                // payload must be a tuple box of exactly n elements (the
                // `payload_box` bundle); a single list value never spreads
                // across parameters (2.1 proposal 1).
                cf.op(I::LocalGet(1));
                cf.op(I::I32Load(ma(0, 2)));
                cf.op(I::I32Const(TAG_TUP));
                cf.op(I::I32Ne);
                cf.op(I::If(BlockType::Empty));
                cf.op(I::Unreachable);
                cf.op(I::End);
                cf.op(I::LocalGet(1));
                cf.op(I::I32Load(ma(4, 2)));
                cf.op(I::I32Const(n as i32));
                cf.op(I::I32Ne);
                cf.op(I::If(BlockType::Empty));
                cf.op(I::Unreachable);
                cf.op(I::End);
                for (i, p) in params.iter().enumerate() {
                    let l = cf.local(ValType::I32);
                    cf.op(I::LocalGet(1));
                    cf.op(I::I32Load(ma(8 + 4 * i as u64, 2)));
                    cf.op(I::LocalSet(l));
                    scope.insert(p.clone(), Binding::boxed(l));
                }
            }
        }
        cf.scopes.push(scope);
        self.expr(&mut cf, body, true)?;
        let t = self.ty_idx(vec![ValType::I32; 2], vec![ValType::I32]);
        self.closure_bodies.push((t, cf.finish()));
        let slot = (self.closure_bodies.len() - 1) as u32;

        let k = caps.len();
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(12 + 4 * k as i32));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(TAG_FN));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(slot as i32));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(k as i32));
        fx.op(I::I32Store(ma(8, 2)));
        for (j, (_, cap)) in caps.iter().enumerate() {
            fx.op(I::LocalGet(p));
            // a goal-5 typed local boxes at the capture seam (closure
            // capture slots hold box pointers)
            match cap.repr {
                Repr::Boxed => fx.op(I::LocalGet(cap.local)),
                Repr::Scalar(kind) => {
                    fx.op(I::LocalGet(cap.local));
                    self.box_scalar(fx, kind);
                }
                Repr::Mem(t) => {
                    let ty = self.mem_tys[t as usize].clone();
                    self.load_from_mem(fx, &ty, cap.local, 0)?;
                }
            }
            fx.op(I::I32Store(ma(12 + 4 * j as u64, 2)));
        }
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// 5.8 capture devirtualization: lambda-lift a qualifying `Fn` literal
    /// (an inline Let init with a pre-reserved index) to a top-level core
    /// function and record it on the binding.
    ///
    /// The lifted function takes the lambda's captures — every currently-visible
    /// local, in the same name-sorted order `fn_form` uses — as leading typed
    /// parameters, followed by its value parameters (boxed, first cut), and
    /// returns a boxed result. Its body is compiled here (captures and params
    /// bound as direct wasm parameters, not env-unpacked) and stashed for the
    /// assembly step to push at `reserved_idx`. Returns the `known_lambdas`
    /// index, or `None` when the node was not pre-reserved (e.g. the macro
    /// component path, or a lambda in an un-scanned body).
    pub(crate) fn compile_known_lambda(
        &mut self,
        fx: &FnCtx,
        fn_node: NodeId,
    ) -> Result<Option<u32>, String> {
        let Some(&reserved_idx) = self.lambda_reserved.get(&fn_node) else {
            return Ok(None);
        };
        let Node::Tup(items) = self.arena.node(fn_node).clone() else {
            return Ok(None);
        };
        if items.len() != 3 {
            return Ok(None);
        }
        let (params_id, body) = (items[1], items[2]);
        let params = param_names(self.arena, params_id)?;

        // captures: every visible local by name (later scopes shadow earlier),
        // sorted so the layout is deterministic — identical to `fn_form`.
        let mut cap_map: HashMap<String, Binding> = HashMap::new();
        for scope in &fx.scopes {
            for (k, &v) in scope {
                cap_map.insert(k.clone(), v);
            }
        }
        let mut caps: Vec<(String, Binding)> = cap_map.into_iter().collect();
        caps.sort_by(|a, b| a.0.cmp(&b.0));
        let ncap = caps.len() as u32;

        let mut cf = FnCtx::new(ncap + params.len() as u32);
        let mut scope = HashMap::new();
        for (i, (cname, cb)) in caps.iter().enumerate() {
            // a capture becomes a direct typed parameter at its current repr
            scope.insert(cname.clone(), Binding::new(i as u32, cb.repr));
        }
        for (j, p) in params.iter().enumerate() {
            scope.insert(p.clone(), Binding::boxed(ncap + j as u32));
        }
        cf.scopes.push(scope);
        self.expr(&mut cf, body, true)?;

        let mut pvts: Vec<ValType> = caps.iter().map(|(_, b)| repr_vt(b.repr)).collect();
        pvts.extend(std::iter::repeat(ValType::I32).take(params.len()));
        let t = self.ty_idx(pvts, vec![ValType::I32]);
        self.lambda_stash.insert(fn_node, (t, cf.finish()));

        let captures: Vec<(Repr, u32)> = caps.iter().map(|(_, b)| (b.repr, b.local)).collect();
        self.known_lambdas.push(KnownLambda {
            reserved_idx,
            captures,
        });
        Ok(Some(self.known_lambdas.len() as u32 - 1))
    }

    /// Indirect call through a closure box: `(box, payload-box)` via the
    /// funcref table slot stored in the box at offset 4.
    pub(crate) fn closure_call(
        &mut self,
        fx: &mut FnCtx,
        head: NodeId,
        args: &[NodeId],
        tail: bool,
    ) -> Result<(), String> {
        self.expr(fx, head, false)?;
        let c = fx.local(ValType::I32);
        fx.op(I::LocalSet(c));
        fx.op(I::LocalGet(c)); // env argument = the closure box itself
        self.payload_box(fx, args)?;
        fx.op(I::LocalGet(c));
        fx.op(I::I32Load(ma(4, 2))); // table slot
        let t = self.ty_idx(vec![ValType::I32; 2], vec![ValType::I32]);
        fx.op(if tail {
            I::ReturnCallIndirect {
                type_index: t,
                table_index: 0,
            }
        } else {
            I::CallIndirect {
                type_index: t,
                table_index: 0,
            }
        });
        Ok(())
    }

    /// Bundle a call's evaluated arguments into one payload box, mirroring the
    /// interpreter's `bundle_args`: 0 args ⇒ the empty tuple, 1 arg ⇒ the value
    /// itself, ≥2 ⇒ a tuple box. The multi-arg bundle is a TAG_TUP (not a list)
    /// so the wrapper can tell an argument bundle apart from a single list value
    /// passed as the sole argument — a list never spreads across parameters
    /// (2.1 proposal 1), matching the interpreter.
    pub(crate) fn payload_box(&mut self, fx: &mut FnCtx, args: &[NodeId]) -> Result<(), String> {
        match args {
            [] => self.seq_box(fx, &[], TAG_TUP),
            [one] => self.expr(fx, *one, false),
            many => self.seq_box(fx, many, TAG_TUP),
        }
    }

    pub(crate) fn call(
        &mut self,
        fx: &mut FnCtx,
        head: NodeId,
        args: &[NodeId],
        tail: bool,
    ) -> Result<(), String> {
        let head_node = self.arena.node(head).clone();
        match head_node {
            Node::Qsym(alias, fname) => {
                // Every imported call goes through the generic canonical-ABI
                // bridge, driven by the import's parsed WIT signature (from a
                // sibling `.wlt` or a `wit/deps` package — host `wasi:*`
                // packages included).
                self.dep_call(fx, &alias, &fname, args, None)
            }
            Node::Sym(name) => match name.as_str() {
                "let-MACRO" => self.let_form(fx, args, Repr::Boxed, tail),
                "the-MACRO" => {
                    // args = [ty, expr]
                    let [_ty, expr] = *args else {
                        return Err("malformed The".into());
                    };
                    self.expr(fx, expr, tail)
                }
                "match-MACRO" => self.match_form(fx, args, Repr::Boxed, tail),
                "fn-MACRO" => self.fn_form(fx, args),
                "quote-MACRO" => {
                    let [form] = args else {
                        return Err("malformed Quote".into());
                    };
                    self.quote_box(fx, *form)
                }
                "quasi-MACRO" => {
                    let [form] = args else {
                        return Err("malformed Quasi".into());
                    };
                    self.quasi_box(fx, *form, 1)
                }
                "def-MACRO" | "defmacro-MACRO" => {
                    Err(format!("`{name}` not supported by the wasm backend yet"))
                }
                _ if fx.lookup(&name).is_some() => {
                    // 5.8 devirtualization: an apply through a binding known to
                    // hold exactly one named def compiles as a direct call —
                    // the same code path a direct `dname(args…)` takes — instead
                    // of bundling a payload and calling through the TAG_FN
                    // table. Semantics agree: the interpreter applies the same
                    // closure either way.
                    if let Some(k) = fx.lookup(&name).and_then(|b| b.known_fn) {
                        let dname = self.known_fn_names[k as usize].clone();
                        return self.internal_call(fx, &dname, args, Repr::Boxed, tail);
                    }
                    // 5.8 capture devirtualization: an apply through a binding
                    // holding a lambda-lifted `Fn` literal pushes the captured
                    // locals (verbatim, at their reprs) and the boxed arguments,
                    // then calls the lifted function directly — no closure box,
                    // no funcref dispatch. Arity is guaranteed by the checker
                    // (`check_indirect_apply`); the lifted body binds each value
                    // parameter boxed, exactly as the boxed closure would.
                    if let Some(kl_idx) = fx.lookup(&name).and_then(|b| b.known_lambda) {
                        let kl = self.known_lambdas[kl_idx as usize].clone();
                        for (_repr, local) in &kl.captures {
                            fx.op(I::LocalGet(*local));
                        }
                        for a in args {
                            self.expr(fx, *a, false)?;
                        }
                        fx.op(if tail {
                            I::ReturnCall(kl.reserved_idx)
                        } else {
                            I::Call(kl.reserved_idx)
                        });
                        return Ok(());
                    }
                    self.closure_call(fx, head, args, tail)
                }
                _ if BUILTINS.contains(&name.as_str()) => self.builtin(fx, &name, args),
                _ => {
                    if self.funcs.contains_key(&name) {
                        self.internal_call(fx, &name, args, Repr::Boxed, tail)
                    } else if self.value_globals.contains_key(&name) {
                        self.closure_call(fx, head, args, tail)
                    } else if let Some(&has_payload) = self.local_cases.get(name.as_str()) {
                        // A `DefType` variant case constructor call (4.1): build
                        // the variant box, bundling ≥2 payload args as a tuple
                        // exactly like the interpreter's `CaseCtor`.
                        if !has_payload || args.is_empty() {
                            return Err(format!(
                                "variant case `{name}` {} (wasm backend)",
                                if has_payload {
                                    "takes a payload, got no arguments"
                                } else {
                                    "is not callable"
                                }
                            ));
                        }
                        self.var_box(fx, &name, args)
                    } else {
                        Err(format!("unknown function `{name}` (wasm backend)"))
                    }
                }
            },
            // any other head evaluates to a closure box
            _ => self.closure_call(fx, head, args, tail),
        }
    }

    pub(crate) fn let_form(
        &mut self,
        fx: &mut FnCtx,
        args: &[NodeId],
        want: Repr,
        tail: bool,
    ) -> Result<(), String> {
        let [bindings, body] = *args else {
            return Err("malformed Let".into());
        };
        let Node::Rec(fields) = self.arena.node(bindings).clone() else {
            return Err("Let bindings must be a record".into());
        };
        fx.scopes.push(HashMap::new());
        for (k, v) in &fields {
            // Goal 5 (5.2): a binding with a statically-known scalar type
            // lives UNBOXED in a typed wasm local; scalar consumers read it
            // directly, boxed consumers box at the reference seam.
            let binding = if let Some(kind) = self.node_scalar(*v) {
                self.expr_scalar(fx, *v, kind)?;
                let l = fx.local(match kind {
                    Scalar::Int | Scalar::Char => ValType::I64,
                    Scalar::Float => ValType::F64,
                    Scalar::Bool => ValType::I32,
                });
                Binding::new(l, Repr::Scalar(kind))
            } else if let Some(t) = self.node_mem(fx, *v) {
                // 5.3: a record binding whose construction is provably
                // faithful to its static type lives in canonical layout;
                // boxed consumers rebuild the box at the reference seam
                self.expr_mem(fx, *v, t, false)?;
                Binding::new(fx.local(ValType::I32), Repr::Mem(t))
            } else {
                self.expr(fx, *v, false)?;
                let mut b = Binding::boxed(fx.local(ValType::I32));
                // 5.8 devirtualization: a binding initialised with a bare
                // module-level def reference whose checked type is a concrete
                // arrow holds exactly that function value — record the def so
                // apply sites through this binding compile to a direct call
                // (the zero-capture defunctionalization case). The boxed
                // closure value is still built: non-apply uses read it.
                if let Node::Sym(dname) = self.arena.node(*v)
                    && self.funcs.contains_key(dname)
                    && matches!(self.node_types.get(v), Some(crate::check::Type::Fn(..)))
                {
                    let idx = self
                        .known_fn_names
                        .iter()
                        .position(|n| n == dname)
                        .unwrap_or_else(|| {
                            self.known_fn_names.push(dname.clone());
                            self.known_fn_names.len() - 1
                        });
                    b.known_fn = Some(idx as u32);
                }
                // 5.8 capture devirtualization: an inline `Fn` literal init
                // with a concrete arrow type is lambda-lifted to a direct-call
                // core function; the boxed closure built above still serves any
                // non-apply use of the binding. (`compile_known_lambda` returns
                // `None` for a non-reserved node, so the two cases are disjoint.)
                if let Some(kl) = self.compile_known_lambda(fx, *v)? {
                    b.known_lambda = Some(kl);
                }
                b
            };
            fx.op(I::LocalSet(binding.local));
            fx.scopes.last_mut().unwrap().insert(k.clone(), binding);
        }
        let r = self.expr_repr(fx, body, want, tail);
        fx.scopes.pop();
        r
    }

    /// Mirror of the interpreter's §4.2 argument-binding rule, at compile time.
    /// `args` are the call's argument forms (`Tup[head, …args]`).
    pub(crate) fn bind_args(&self, args: &[NodeId], params: &[String]) -> Result<BoundArgs, String> {
        // named: a single record arg whose keys are exactly the parameters
        if let [only] = args
            && let Node::Rec(fields) = self.arena.node(*only)
        {
            let mut keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
            let mut want: Vec<&str> = params.iter().map(|s| s.as_str()).collect();
            keys.sort();
            want.sort();
            if keys == want {
                let map: HashMap<&str, NodeId> =
                    fields.iter().map(|(k, v)| (k.as_str(), *v)).collect();
                return Ok(BoundArgs::PerParam(
                    params.iter().map(|p| map[p.as_str()]).collect(),
                ));
            }
        }
        // positional: one arg per parameter (covers the scalar 1/1 and 0/0 cases)
        if args.len() == params.len() {
            return Ok(BoundArgs::PerParam(args.to_vec()));
        }
        // a sole parameter receives the whole bundle as a tuple
        if params.len() == 1 {
            return Ok(BoundArgs::Bundle);
        }
        Err(format!(
            "payload does not match parameters ({})",
            params.join(", ")
        ))
    }

    /// Call an internal def, producing the result in representation `want`
    /// (`None` = boxed). Arguments are emitted per the callee's repr
    /// signature — typed slots receive unboxed scalars directly. A tail call
    /// stays a `return_call` when caller and callee agree on the result
    /// representation; a representation seam (box/unbox/convert after the
    /// call) forces a plain call.
    pub(crate) fn internal_call(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        args: &[NodeId],
        want: Repr,
        tail: bool,
    ) -> Result<(), String> {
        let (idx, params, sig) = self.funcs[name].clone();
        match self.bind_args(args, &params)? {
            BoundArgs::PerParam(nodes) => {
                for (a, slot) in nodes.into_iter().zip(sig.params.iter()) {
                    self.expr_repr(fx, a, *slot, false)?;
                }
            }
            BoundArgs::Bundle => {
                self.seq_box(fx, args, TAG_TUP)?;
                // sole parameter; a typed slot unboxes the bundle — trapping,
                // as the interpreter's use of a mis-bound bundle would error
                if let [Repr::Scalar(k)] = sig.params[..] {
                    self.unbox_scalar(fx, k);
                }
            }
        }
        if tail && want == sig.result {
            fx.op(I::ReturnCall(idx));
            return Ok(());
        }
        fx.op(I::Call(idx));
        // representation seam on the result
        match (sig.result, want) {
            (a, b) if a == b => {}
            (Repr::Scalar(Scalar::Int), Repr::Scalar(Scalar::Float)) => {
                fx.op(I::F64ConvertI64S);
            }
            (Repr::Scalar(k), Repr::Boxed) => self.box_scalar(fx, k),
            (Repr::Boxed, Repr::Scalar(k)) => self.unbox_scalar(fx, k),
            (Repr::Mem(t), Repr::Boxed) => {
                let l = fx.local(ValType::I32);
                fx.op(I::LocalSet(l));
                let ty = self.mem_tys[t as usize].clone();
                self.load_from_mem(fx, &ty, l, 0)?;
            }
            (Repr::Mem(t), Repr::Scalar(k)) => {
                // rebuild the box, then unbox: traps exactly where the boxed
                // path would (a record where a scalar is required)
                let l = fx.local(ValType::I32);
                fx.op(I::LocalSet(l));
                let ty = self.mem_tys[t as usize].clone();
                self.load_from_mem(fx, &ty, l, 0)?;
                self.unbox_scalar(fx, k);
            }
            (a, b) => {
                return Err(format!(
                    "internal: `{name}` returns {a:?} where {b:?} is expected"
                ));
            }
        }
        Ok(())
    }

    /// The [`Dep`] an import alias resolves to, if it names one in the build set.
    pub(crate) fn dep_for_alias(&self, alias: &str) -> Result<&Dep, String> {
        let imp = self
            .info
            .imports
            .iter()
            .find(|i| i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        self.deps.get(&imp.package).ok_or(format!(
            "dependency `{}` is not in the build set",
            imp.package
        ))
    }

    pub(crate) fn dep_call(
        &mut self,
        fx: &mut FnCtx,
        alias: &str,
        fname: &str,
        args: &[NodeId],
        want_mem: Option<MemTy>,
    ) -> Result<(), String> {
        // A functor op (`pts/new`, `pts/add`, `pts/contains`, `pts/size`) is not a
        // runtime import: it routes to the locally-emitted `set` resource's core
        // funcs (`ResourceFns`, indices reserved in `em.functor_fns`). The mapping
        // mirrors the interpreter's `bind_functor` (`builtins.rs`): `new`→ctor,
        // `add`/`contains`/`size`→methods.
        //
        // HANDLE CONVENTION (step 04 ABI decision; see `summaries/04-routing.typ`):
        // a `set` Wavelet value carries the OWN handle minted by the constructor's
        // `resource.new`, boxed as an int box (TAG_INT) — the same opaque-handle
        // carriage `lower`/`lift` already use for `WitTy::Handle`, so the export
        // boundary (`nearest-set -> own<set>`) needs no special-casing: it just
        // unboxes the i32. A method's core body, however, receives the REP as
        // `self` (summary 01: the canonical ABI hands an exported resource's method
        // the rep directly). So intra-guest we convert handle→rep with the
        // `[resource-rep]set` intrinsic before each method call. (This is the
        // alternative to carrying the rep guest-side and minting at the boundary;
        // it keeps `lower`/`lift` untouched and reuses the ctor verbatim.)
        if let Some(inst) = self.info.functors.iter().find(|f| f.alias == alias) {
            let fns = *self.functor_fns.get(alias).ok_or_else(|| {
                format!("internal error: no emitted `set` resource for functor alias `{alias}`")
            })?;
            match fname {
                "new" => {
                    if !args.is_empty() {
                        return Err(format!("`{alias}/new` takes no arguments"));
                    }
                    fx.op(I::Call(fns.ctor)); // () -> i32 own handle
                    self.lift(fx, &WitTy::Handle); // box the handle as the set value
                }
                "add" | "contains" | "size" => {
                    let want = if fname == "size" { 1 } else { 2 };
                    if args.len() != want {
                        return Err(format!(
                            "`{alias}/{fname}` takes {want} argument{}",
                            if want == 1 { "" } else { "s" }
                        ));
                    }
                    // arg 0 is the set: unbox its handle, recover the rep (the
                    // method's `self`), and stash it below the flattened value args.
                    let rep = fx.local(ValType::I32);
                    self.expr(fx, args[0], false)?;
                    fx.op(I::Call(self.h.unbox_int));
                    fx.op(I::I32WrapI64);
                    fx.op(I::Call(fns.rep_import)); // handle -> rep
                    fx.op(I::LocalSet(rep));
                    fx.op(I::LocalGet(rep));
                    if fname != "size" {
                        // the element value, flattened in canonical-ABI order
                        let elem = wit_ty(&inst.elem, &self.type_env)?;
                        self.expr(fx, args[1], false)?;
                        self.lower(fx, &elem)?;
                    }
                    match fname {
                        "add" => {
                            // `set-add` returns unit in the oracle
                            // (`Value::Rec(vec![])`, printing "{}"), so yield
                            // the interned empty-record box, not the false-box.
                            let unit = self.intern_unit_rec();
                            fx.op(I::Call(fns.add)); // (rep, <elem>) -> ()
                            fx.op(I::I32Const(unit as i32));
                        }
                        "contains" => {
                            fx.op(I::Call(fns.contains)); // -> i32 0/1
                            self.lift(fx, &WitTy::Bool);
                        }
                        _ => {
                            fx.op(I::Call(fns.size)); // -> i32 u32
                            self.lift(fx, &WitTy::IntU(4));
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "functor `set` (alias `{alias}`) has no op `{other}`; \
                         expected new / add / contains / size"
                    ));
                }
            }
            return Ok(());
        }
        // A user-declared resource (4.5): `counter/next(c)` (method) or
        // `counter/sum(vs)` (static). A resource value is carried guest-internally
        // as its rep (the `New` cell), so `self` is simply the first argument — a
        // method or static is therefore an ordinary internal call. The own/borrow
        // handle conversions happen only at the component boundary.
        if let Some(ur) = self.user_res.get(alias).cloned() {
            let mname = format!("{alias}/{fname}");
            if ur.methods.contains(fname) || ur.statics.contains(fname) {
                return self.internal_call(fx, &mname, args, Repr::Boxed, false);
            }
            return Err(format!("resource `{alias}` has no member `{fname}`"));
        }
        let imp = self
            .info
            .imports
            .iter()
            .find(|i| i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        let dep = self.deps.get(&imp.package).ok_or(format!(
            "dependency `{}` is not in the build set",
            imp.package
        ))?;
        let iface = import_iface(&imp.path);
        // Resolve freestanding names directly, and resource operations
        // (`[method]`/`[static]`/`[constructor]`/`[resource-drop]`) by their
        // bare op name. A name that is no function but IS a case of one of the
        // dep's variant/enum types is a case constructor call (4.1).
        let sig = match resolve_dep_func(dep, &iface, fname) {
            Ok(sig) => sig.clone(),
            Err(e) => {
                return match dep_case(dep, fname) {
                    Some(true) if !args.is_empty() => self.var_box(fx, fname, args),
                    Some(true) => Err(format!(
                        "variant case `{alias}/{fname}` takes a payload, got no arguments"
                    )),
                    Some(false) if args.is_empty() => {
                        let addr = self.none_like_box(fname);
                        fx.op(I::I32Const(addr as i32));
                        Ok(())
                    }
                    Some(false) => Err(format!(
                        "variant case `{alias}/{fname}` is not callable (use the bare name)"
                    )),
                    None => Err(e),
                };
            }
        };
        let module = versioned_iface(&dep.package, &iface);
        // The host import is keyed by the *mangled* WIT name (`sig.name`), which
        // is what the import-signature loop declares and what `wit-component`
        // re-validates against the WIT.
        let fidx = self.import_idx(&module, &sig.name);

        let param_names: Vec<String> = sig.params.iter().map(|(n, _)| n.clone()).collect();
        let arg_nodes = match self.bind_args(args, &param_names)? {
            BoundArgs::PerParam(nodes) => nodes,
            BoundArgs::Bundle => {
                // 5.10: several arguments bundle into the sole parameter as a
                // tuple value — exactly how the interpreter binds a call's
                // payload to one parameter. Build the tuple box and lower it
                // against the parameter's (tuple) WIT type here; the empty
                // node list below then has nothing left to lower.
                self.seq_box(fx, args, TAG_TUP)?;
                let (_, t) = &sig.params[0];
                let ty = wit_ty(t, &self.type_env)?;
                self.lower(fx, &ty)?;
                Vec::new()
            }
        };
        for (a, (_, t)) in arg_nodes.iter().zip(&sig.params) {
            self.expr(fx, *a, false)?;
            let pty = wit_ty(t, &self.type_env)?;
            self.lower(fx, &pty)?;
        }
        if want_mem.is_some() && !matches!(flat_result(&sig, &self.type_env), Ok(FlatRes::Retptr)) {
            return Err("internal: Mem repr requested for a non-retptr dep call (5.3)".into());
        }
        match flat_result(&sig, &self.type_env)? {
            FlatRes::None => {
                // A no-result dep call yields unit (`{}`), matching the
                // interpreter — the false-box would print "false".
                let unit = self.intern_unit_rec();
                fx.op(I::Call(fidx));
                fx.op(I::I32Const(unit as i32));
            }
            FlatRes::One(t) => {
                fx.op(I::Call(fidx));
                self.lift(fx, &t);
            }
            FlatRes::Retptr => {
                let rty = wit_ty(sig.result.as_deref().unwrap(), &self.type_env)?;
                if matches!(
                    rty,
                    WitTy::Record(_)
                        | WitTy::Tuple(_)
                        | WitTy::Option(_)
                        | WitTy::Result(..)
                        | WitTy::Variant(_)
                ) || (want_mem.is_some() && matches!(rty, WitTy::Str | WitTy::List(_)))
                {
                    // allocate a result area sized to the value, pass it as the
                    // canonical retptr, then read the value back out of it —
                    // or, when the caller wants this canonical layout (5.3),
                    // the area IS the value: no lift, no boxes. A string/list
                    // result's canonical form is the (ptr, len) pair the area
                    // carries (5.5), so the same fast path applies.
                    let area = fx.local(ValType::I32);
                    fx.op(I::I32Const(size_of(&rty) as i32));
                    fx.op(I::Call(self.h.alloc));
                    fx.op(I::LocalTee(area));
                    fx.op(I::Call(fidx));
                    match want_mem {
                        Some(t) if self.mem_tys[t as usize] == rty => {
                            fx.op(I::LocalGet(area));
                        }
                        Some(_) => {
                            return Err(
                                "internal: dep call's canonical layout does not match                                  the requested Mem type (5.3)"
                                    .into(),
                            );
                        }
                        None => self.load_from_mem(fx, &rty, area, 0)?,
                    }
                } else {
                    fx.op(I::I32Const(SCRATCH));
                    fx.op(I::Call(fidx));
                    // (ptr, len) written at the scratch area
                    let p = fx.local(ValType::I32);
                    let l = fx.local(ValType::I32);
                    fx.op(I::I32Const(SCRATCH));
                    fx.op(I::I32Load(ma(0, 2)));
                    fx.op(I::LocalSet(p));
                    fx.op(I::I32Const(SCRATCH));
                    fx.op(I::I32Load(ma(4, 2)));
                    fx.op(I::LocalSet(l));
                    match rty {
                        WitTy::List(elem) => self.lift_list(fx, p, l, &elem)?,
                        _ => {
                            fx.op(I::LocalGet(p));
                            fx.op(I::LocalGet(l));
                            fx.op(I::Call(self.h.box_str));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
