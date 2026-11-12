use super::*;

impl<'a> Emitter<'a> {
    pub(crate) fn fn_form(&mut self, fx: &mut FnCtx, args: &[NodeId]) -> Result<(), String> {
        let [params_id, body] = *args else {
            return Err("malformed Fn".into());
        };
        let params = param_names(self.arena, params_id)?;

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
            scope.insert(cname.clone(), Binding::new(i as u32, cb.repr));
        }
        for (j, p) in params.iter().enumerate() {
            scope.insert(p.clone(), Binding::boxed(ncap + j as u32));
        }
        cf.scopes.push(scope);
        self.expr(&mut cf, body, true)?;

        let mut pvts: Vec<ValType> = caps.iter().map(|(_, b)| repr_vt(b.repr)).collect();
        pvts.extend(std::iter::repeat_n(ValType::I32, params.len()));
        let t = self.ty_idx(pvts, vec![ValType::I32]);
        self.lambda_stash.insert(fn_node, (t, cf.finish()));

        let captures: Vec<(Repr, u32)> = caps.iter().map(|(_, b)| (b.repr, b.local)).collect();
        self.known_lambdas.push(KnownLambda {
            reserved_idx,
            captures,
        });
        Ok(Some(self.known_lambdas.len() as u32 - 1))
    }

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
        fx.op(I::LocalGet(c));
        self.payload_box(fx, args)?;
        fx.op(I::LocalGet(c));
        fx.op(I::I32Load(ma(4, 2)));
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
            Node::Sym(name) => match name.as_str() {
                "quote-MACRO" | "quasi-MACRO" if self.macro_expand_idx.is_none() => Err(
                    "source forms are only available at compile time inside macro bodies".into(),
                ),
                "let-MACRO" => self.let_form(fx, args, Repr::Boxed, tail),
                "the-MACRO" => {
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
                    if let Some(k) = fx.lookup(&name).and_then(|b| b.known_fn) {
                        let dname = self.known_fn_names[k as usize].clone();
                        return self.internal_call(fx, &dname, args, Repr::Boxed, tail);
                    }

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
                _ => {
                    if self.funcs.contains_key(&name) {
                        self.internal_call(fx, &name, args, Repr::Boxed, tail)
                    } else if self.value_globals.contains_key(&name) {
                        self.closure_call(fx, head, args, tail)
                    } else if let Some(&has_payload) = self.local_cases.get(name.as_str()) {
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
                    } else if let Some((alias, function)) =
                        crate::wit::qualified_name(self.info, &name)
                    {
                        self.dep_call(fx, &alias, &function, args, None)
                    } else if BUILTINS.contains(&name.as_str()) {
                        self.builtin(fx, &name, args)
                    } else {
                        Err(format!("unknown function `{name}` (wasm backend)"))
                    }
                }
            },

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
            let binding = if let Some(kind) = self.node_scalar(*v) {
                self.expr_scalar(fx, *v, kind)?;
                let l = fx.local(match kind {
                    Scalar::Int | Scalar::Char => ValType::I64,
                    Scalar::Float => ValType::F64,
                    Scalar::Bool => ValType::I32,
                });
                Binding::new(l, Repr::Scalar(kind))
            } else if let Some(t) = self.node_mem(fx, *v) {
                self.expr_mem(fx, *v, t, false)?;
                Binding::new(fx.local(ValType::I32), Repr::Mem(t))
            } else {
                self.expr(fx, *v, false)?;
                let mut b = Binding::boxed(fx.local(ValType::I32));

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

    pub(crate) fn bind_args(
        &self,
        args: &[NodeId],
        params: &[String],
    ) -> Result<BoundArgs, String> {
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

        if args.len() == params.len() {
            return Ok(BoundArgs::PerParam(args.to_vec()));
        }

        if params.len() == 1 {
            return Ok(BoundArgs::Bundle);
        }
        Err(format!(
            "payload does not match parameters ({})",
            params.join(", ")
        ))
    }

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
        if let Some(inst) = self.info.functors.iter().find(|f| f.alias == alias) {
            let fns = *self.functor_fns.get(alias).ok_or_else(|| {
                format!("internal error: no emitted `set` resource for functor alias `{alias}`")
            })?;
            match fname {
                "new" => {
                    if !args.is_empty() {
                        return Err(format!("`{alias}/new` takes no arguments"));
                    }
                    fx.op(I::Call(fns.ctor));
                    self.lift(fx, &WitTy::Handle);
                }
                "add" | "contains" | "size" => {
                    let want = if fname == "size" { 1 } else { 2 };
                    if args.len() != want {
                        return Err(format!(
                            "`{alias}/{fname}` takes {want} argument{}",
                            if want == 1 { "" } else { "s" }
                        ));
                    }

                    let rep = fx.local(ValType::I32);
                    self.expr(fx, args[0], false)?;
                    fx.op(I::Call(self.h.unbox_int));
                    fx.op(I::I32WrapI64);
                    fx.op(I::Call(fns.rep_import));
                    fx.op(I::LocalSet(rep));
                    fx.op(I::LocalGet(rep));
                    if fname != "size" {
                        let elem = wit_ty(&inst.elem, &self.type_env)?;
                        self.expr(fx, args[1], false)?;
                        self.lower(fx, &elem)?;
                    }
                    match fname {
                        "add" => {
                            fx.op(I::Call(fns.add));
                            fx.op(I::I32Const(self.unit_addr() as i32));
                        }
                        "contains" => {
                            fx.op(I::Call(fns.contains));
                            self.lift(fx, &WitTy::Bool);
                        }
                        _ => {
                            fx.op(I::Call(fns.size));
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

        if let Some(ur) = self.user_res.get(alias).cloned() {
            let mname = format!("{alias}-{fname}");
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

        let sig = match resolve_dep_func(dep, &iface, fname) {
            Ok(sig) => sig.clone(),
            Err(e) => {
                return self.dep_case_call(fx, alias, fname, args, dep_case(dep, fname), e);
            }
        };
        let module = versioned_iface(&dep.package, &iface);

        let fidx = self.import_idx(&module, &sig.name);

        let param_names: Vec<String> = sig.params.iter().map(|(n, _)| n.clone()).collect();
        let arg_nodes = match self.bind_args(args, &param_names)? {
            BoundArgs::PerParam(nodes) => nodes,
            BoundArgs::Bundle => {
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
                fx.op(I::Call(fidx));
                fx.op(I::I32Const(self.unit_addr() as i32));
            }
            FlatRes::One(t) => {
                fx.op(I::Call(fidx));
                let fields = flat(&t);
                let result = fx.local(fields.first().copied().unwrap_or(ValType::I32));
                if !fields.is_empty() {
                    fx.op(I::LocalSet(result));
                }
                self.lift_flat(fx, &t, result)?;
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

    fn dep_case_call(
        &mut self,
        fx: &mut FnCtx,
        alias: &str,
        fname: &str,
        args: &[NodeId],
        payload: Option<bool>,
        unresolved: String,
    ) -> Result<(), String> {
        match payload {
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
            None => Err(unresolved),
        }
    }
}
