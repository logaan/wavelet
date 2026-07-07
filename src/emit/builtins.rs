//! `Emitter` builtin dispatch and the per-group builtin emitters, plus the
//! [`BUILTINS`] name list the call path routes through.

use super::*;

impl<'a> Emitter<'a> {
    pub(crate) fn builtin(&mut self, fx: &mut FnCtx, name: &str, args: &[NodeId]) -> Result<(), String> {
        // Goal 5 (5.2/5.6.1): when the operand types are statically known
        // scalars, compile the polymorphic builtin as unboxed per-type code
        // and box once at the seam. Ineligible calls (unknown/compound
        // operand types, strings, arity errors) fall through to the boxed
        // arms below unchanged.
        if matches!(
            name,
            "eq" | "not"
                | "lt"
                | "le"
                | "gt"
                | "ge"
                | "add"
                | "sub"
                | "mul"
                | "div"
                | "rem"
                | "neg"
                | "to-u8"
                | "to-u16"
                | "to-u32"
                | "to-u64"
                | "to-s8"
                | "to-s16"
                | "to-s32"
                | "to-s64"
                | "to-f32"
                | "to-f64"
                | "to-char"
        ) && let Some(kind) = self.scalar_op(fx, name, args)?
        {
            self.box_scalar(fx, kind);
            return Ok(());
        }
        let items = args;
        // Boxed-path dispatch: route each builtin to the helper that owns its
        // group. Every arm below emits the polymorphic, box-once code for its
        // family; the helpers share the `nargs` arity check and operate on the
        // `items` operand slice. Adding a builtin means extending both this
        // classifier and the relevant `builtin_*` method (plus `BUILTINS`).
        match name {
            "eq" | "not" | "lt" | "le" | "gt" | "ge" | "add" | "sub" | "mul" | "div" | "rem" | "neg" | "abs" | "min" | "max" => self.builtin_numeric(fx, name, items),
            "len" | "head" | "tail" | "reverse" | "range" | "empty" | "drop" => self.builtin_seq(fx, name, items),
            "map" | "fold" | "filter" | "zip" | "apply" => self.builtin_higher_order(fx, name, items),
            "get" | "put" | "push" | "concat" => self.builtin_index(fx, name, items),
            "contains" | "join" | "split" => self.builtin_search(fx, name, items),
            "str-cat" | "to-string" | "to-char" | "upper" | "lower" => self.builtin_string(fx, name, items),
            "some" | "ok" | "err" => self.builtin_variant(fx, name, items),
            "cell-new" | "cell-get" | "cell-set" => self.builtin_cell(fx, name, items),
            "form-kind" | "rec-key" | "rec-val" | "gensym" | "expand" => self.builtin_form(fx, name, items),
            other => Err(format!(
                "builtin `{other}` not supported by the wasm backend yet"
            )),
        }
    }

    /// Comparison and arithmetic builtins: `eq`/`not`, the ordering
    /// comparisons, the binary arithmetic ops, `neg`/`abs`, and `min`/`max`.
    /// These are the boxed fallbacks for calls the unboxed scalar fast path
    /// in `builtin` did not claim (unknown/compound operands, arity errors).
    pub(crate) fn builtin_numeric(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "eq" => {
                nargs(2)?;
                // Type-indexed structural eq (5.6): when both operands are the
                // SAME statically-canonical Mem layout, compare their canonical
                // representations field-by-field instead of reboxing both and
                // calling `eq_raw`. A reordered/renamed record literal never
                // reaches the same MemTy (`can_mem_as` requires declaration
                // order), so field-order-sensitive `eq` stays correct; flags
                // and handles are ineligible (see `mem_eq_eligible`).
                if let (Some(ta), Some(tb)) =
                    (self.node_mem(fx, items[0]), self.node_mem(fx, items[1]))
                    && ta == tb
                {
                    let ty = self.mem_tys[ta as usize].clone();
                    if self.mem_eq_eligible(&ty) {
                        let pa = fx.local(ValType::I32);
                        self.expr_mem(fx, items[0], ta, false)?;
                        fx.op(I::LocalSet(pa));
                        let pb = fx.local(ValType::I32);
                        self.expr_mem(fx, items[1], tb, false)?;
                        fx.op(I::LocalSet(pb));
                        self.emit_mem_eq(fx, &ty, pa, pb, 0)?;
                        fx.op(I::Call(self.h.box_bool));
                        return Ok(());
                    }
                }
                self.expr(fx, items[0], false)?;
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.eq_raw));
                fx.op(I::Call(self.h.box_bool));
            }
            "not" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.truthy));
                fx.op(I::I32Eqz);
                fx.op(I::Call(self.h.box_bool));
            }
            "lt" | "le" | "gt" | "ge" => {
                // cmp_raw yields -1/0/1 over ints, decs, strings and chars,
                // matching the interpreter's `compare`.
                nargs(2)?;
                self.expr(fx, items[0], false)?;
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.cmp_raw));
                fx.op(I::I32Const(0));
                fx.op(match name {
                    "lt" => I::I32LtS,
                    "le" => I::I32LeS,
                    "gt" => I::I32GtS,
                    _ => I::I32GeS,
                });
                fx.op(I::Call(self.h.box_bool));
            }
            "add" | "sub" | "mul" | "div" | "rem" => {
                // strictly binary, like the interpreter's `args_n(arg, 2)`;
                // arith_raw dispatches int (checked) vs float at runtime.
                nargs(2)?;
                self.expr(fx, items[0], false)?;
                self.expr(fx, items[1], false)?;
                fx.op(I::I32Const(match name {
                    "add" => 0,
                    "sub" => 1,
                    "mul" => 2,
                    "div" => 3,
                    _ => 4,
                }));
                fx.op(I::Call(self.h.arith_raw));
            }
            "neg" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.neg_raw));
            }
            "abs" => {
                // |n| over an int (branch: n<0 ? -n : n) or a dec (F64Abs);
                // any other tag traps, matching the oracle's `want_num`.
                nargs(1)?;
                let b = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(b));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_INT));
                fx.op(I::I32Eq);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                let n = fx.local(ValType::I64);
                fx.op(I::LocalGet(b));
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::LocalTee(n));
                fx.op(I::I64Const(0));
                fx.op(I::I64LtS);
                fx.op(I::If(BlockType::Result(ValType::I64)));
                fx.op(I::I64Const(0));
                fx.op(I::LocalGet(n));
                fx.op(I::I64Sub);
                fx.op(I::Else);
                fx.op(I::LocalGet(n));
                fx.op(I::End);
                fx.op(I::Call(self.h.box_int));
                fx.op(I::Else);
                // must be a dec, else trap (want_num)
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_DEC));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(b));
                fx.op(I::F64Load(ma(8, 3)));
                fx.op(I::F64Abs);
                fx.op(I::Call(self.h.box_dec));
                fx.op(I::End);
            }
            "min" | "max" => {
                // The oracle: `min` returns a[1] when compare(a,b)==Greater else
                // a[0]; `max` returns a[1] when compare(a,b)==Less else a[0].
                // `cmp_raw` yields 1 for Greater, -1 for Less over the same total
                // order (ints, decs, strings, chars).
                nargs(2)?;
                let av = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(av));
                let bv = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(bv));
                fx.op(I::LocalGet(av));
                fx.op(I::LocalGet(bv));
                fx.op(I::Call(self.h.cmp_raw));
                fx.op(I::I32Const(if name == "min" { 1 } else { -1 }));
                fx.op(I::I32Eq);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                fx.op(I::LocalGet(bv));
                fx.op(I::Else);
                fx.op(I::LocalGet(av));
                fx.op(I::End);
            }
            _ => unreachable!("builtin_numeric routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Sequence access and construction: `len`/`head`/`tail`, `reverse`,
    /// `range`, the `empty` predicate, and `drop` (evaluate for effect →
    /// unit).
    pub(crate) fn builtin_seq(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "len" => {
                nargs(1)?;
                // Type-indexed (5.6): a statically-canonical LIST operand's
                // length IS the len word of its (ptr, len) area — read it
                // directly instead of reboxing and walking `len_raw`. Only
                // lists qualify: a string's `len` is its CHAR count (not the
                // byte length the word stores), and a tuple's canonical layout
                // is inline fields (no len word) — both keep the boxed path.
                if let Some(mt) = self.node_mem(fx, items[0])
                    && matches!(self.mem_tys[mt as usize], WitTy::List(_))
                {
                    self.expr_mem(fx, items[0], mt, false)?;
                    fx.op(I::I32Load(ma(4, 2)));
                    fx.op(I::I64ExtendI32U);
                    fx.op(I::Call(self.h.box_int));
                    return Ok(());
                }
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.len_raw));
                fx.op(I::I64ExtendI32U);
                fx.op(I::Call(self.h.box_int));
            }
            "head" => {
                nargs(1)?;
                // Type-indexed (5.6): a statically-canonical LIST operand's
                // first element loads straight from its (ptr, len) area — no
                // reboxing the whole list. Matches the oracle: `head` of an
                // empty list traps; otherwise it yields element[0] at its
                // natural boxed repr (exactly what `load_from_mem` produces at
                // the reference seam).
                if let Some(mt) = self.node_mem(fx, items[0])
                    && let WitTy::List(elem) = self.mem_tys[mt as usize].clone()
                {
                    let area = fx.local(ValType::I32);
                    self.expr_mem(fx, items[0], mt, false)?;
                    fx.op(I::LocalSet(area));
                    // trap on the empty list, like the boxed `head_h`
                    fx.op(I::LocalGet(area));
                    fx.op(I::I32Load(ma(4, 2)));
                    fx.op(I::I32Eqz);
                    fx.op(I::If(BlockType::Empty));
                    fx.op(I::Unreachable);
                    fx.op(I::End);
                    let dataptr = fx.local(ValType::I32);
                    fx.op(I::LocalGet(area));
                    fx.op(I::I32Load(ma(0, 2)));
                    fx.op(I::LocalSet(dataptr));
                    self.load_from_mem(fx, &elem, dataptr, 0)?;
                    return Ok(());
                }
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.head_h));
            }
            "tail" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.tail_h));
            }
            "reverse" => {
                // Fresh list with the elements in reverse order (oracle: want_list
                // then reverse). A non-list operand traps.
                nargs(1)?;
                let lst = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(lst));
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(n));
                fx.op(I::I32Store(ma(4, 2)));
                let i = fx.local(ValType::I32);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(1));
                fx.op(I::I32Sub);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Sub);
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(out));
            }
            "range" => {
                // The half-open list [lo, hi) of ints (oracle: (lo..hi).map(Int)).
                // Empty when hi <= lo.
                nargs(2)?;
                let lo = fx.local(ValType::I64);
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::LocalSet(lo));
                let hi = fx.local(ValType::I64);
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::LocalSet(hi));
                let cnt = fx.local(ValType::I32);
                fx.op(I::LocalGet(hi));
                fx.op(I::LocalGet(lo));
                fx.op(I::I64GtS);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                fx.op(I::LocalGet(hi));
                fx.op(I::LocalGet(lo));
                fx.op(I::I64Sub);
                fx.op(I::I32WrapI64);
                fx.op(I::Else);
                fx.op(I::I32Const(0));
                fx.op(I::End);
                fx.op(I::LocalSet(cnt));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(cnt));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(cnt));
                fx.op(I::I32Store(ma(4, 2)));
                let i = fx.local(ValType::I32);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(cnt));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(lo));
                fx.op(I::LocalGet(i));
                fx.op(I::I64ExtendI32S);
                fx.op(I::I64Add);
                fx.op(I::Call(self.h.box_int));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(out));
            }
            "empty" => {
                // true iff a string/list/tuple has length 0 (the len word @4);
                // any other tag traps, like the oracle's non-sequence error.
                nargs(1)?;
                let b = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(b));
                let tg = fx.local(ValType::I32);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::LocalTee(tg));
                fx.op(I::I32Const(TAG_STR));
                fx.op(I::I32Eq);
                fx.op(I::LocalGet(tg));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Eq);
                fx.op(I::I32Or);
                fx.op(I::LocalGet(tg));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Eq);
                fx.op(I::I32Or);
                fx.op(I::I32Eqz);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::I32Eqz);
                fx.op(I::Call(self.h.box_bool));
            }
            "drop" => {
                // Evaluates its operand(s) for effect and yields unit — the
                // interpreter's `drop`. Unit is `Value::Rec(vec![])` (prints as
                // "{}"), so return a static empty-record box rather than the
                // `unit_addr()` false-box (which `to-string` would render
                // "false"); at a no-result WIT boundary the box is discarded.
                for &x in items {
                    self.expr(fx, x, false)?;
                    fx.op(I::Drop);
                }
                let addr = self.intern_unit_rec();
                fx.op(I::I32Const(addr as i32));
            }
            _ => unreachable!("builtin_seq routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Builtins that take a callable or combine whole sequences: `map`,
    /// `fold`, `filter`, `zip`, and `apply`.
    pub(crate) fn builtin_higher_order(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "map" => {
                // map(f, list) -> list: apply the function value `f` to each
                // element, collecting results in source order (oracle:
                // `for v in lst { out.push(interp.apply(&f, v)) }`). Length is
                // preserved, so the result TAG_LIST box is pre-sized and filled.
                // `f` is applied through the boxed-closure convention
                // (`closure_call`): call_indirect(env=closure, payload=elem,
                // slot=closure[4]) — a single argument passes the value itself as
                // the payload, exactly as `payload_box`'s one-arg case does.
                nargs(2)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?; // closure box for f
                fx.op(I::LocalSet(fp));
                let lp = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?; // TAG_LIST box
                fx.op(I::LocalSet(lp));
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                // out = alloc(8 + 4*n); [TAG_LIST, n, _ …]
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(n));
                fx.op(I::I32Store(ma(4, 2)));
                // for i in 0..n: out[8+4i] = apply(f, lp[8+4i])
                let i = fx.local(ValType::I32);
                let apply_ty = self.ty_idx(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // dst = out + 8 + 4*i
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                // env = f
                fx.op(I::LocalGet(fp));
                // payload = lp[8+4*i]
                fx.op(I::LocalGet(lp));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                // slot = f[4]
                fx.op(I::LocalGet(fp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::CallIndirect {
                    type_index: apply_ty,
                    table_index: 0,
                });
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End); // loop
                fx.op(I::End); // block
                fx.op(I::LocalGet(out));
            }
            "fold" => {
                // fold(f, acc, list) -> acc: left fold. Each step applies `f`
                // to the two-element tuple (acc, elem) — the bundle shape the
                // interpreter uses (`apply(f, Tup([acc, elem]))`) — via the
                // boxed-closure convention; the closure wrapper unpacks the
                // TAG_TUP payload into the function's two params.
                nargs(3)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?; // closure box for f
                fx.op(I::LocalSet(fp));
                let acc = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?; // initial accumulator box
                fx.op(I::LocalSet(acc));
                let lp = fx.local(ValType::I32);
                self.expr(fx, items[2], false)?; // TAG_LIST box
                fx.op(I::LocalSet(lp));
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                let i = fx.local(ValType::I32);
                let pay = fx.local(ValType::I32);
                let apply_ty = self.ty_idx(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // pay = [TAG_TUP, 2, acc, lp[8+4*i]]
                fx.op(I::I32Const(16));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(pay));
                fx.op(I::LocalGet(pay));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(pay));
                fx.op(I::I32Const(2));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(pay));
                fx.op(I::LocalGet(acc));
                fx.op(I::I32Store(ma(8, 2)));
                fx.op(I::LocalGet(pay));
                fx.op(I::LocalGet(lp));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Store(ma(12, 2)));
                // acc = apply(f, pay)
                fx.op(I::LocalGet(fp));
                fx.op(I::LocalGet(pay));
                fx.op(I::LocalGet(fp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::CallIndirect {
                    type_index: apply_ty,
                    table_index: 0,
                });
                fx.op(I::LocalSet(acc));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End); // loop
                fx.op(I::End); // block
                fx.op(I::LocalGet(acc));
            }
            "filter" => {
                // filter(f, list) -> list: keep the elements for which the
                // predicate `f` returns true, in order (oracle: a non-bool
                // result is an error). The result is at most as long as the
                // input, so a full-size TAG_LIST is over-allocated, the kept
                // elements packed in, and the length set to the kept count.
                nargs(2)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?; // closure box
                fx.op(I::LocalSet(fp));
                let lp = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?; // TAG_LIST box
                fx.op(I::LocalSet(lp));
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                // out = alloc(8 + 4*n); [TAG_LIST, <k set at end>, …]
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                let i = fx.local(ValType::I32);
                let k = fx.local(ValType::I32);
                let elem = fx.local(ValType::I32);
                let r = fx.local(ValType::I32);
                let apply_ty = self.ty_idx(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(k));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // elem = lp[8+4*i]
                fx.op(I::LocalGet(lp));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::LocalSet(elem));
                // r = apply(f, elem)
                fx.op(I::LocalGet(fp));
                fx.op(I::LocalGet(elem));
                fx.op(I::LocalGet(fp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::CallIndirect {
                    type_index: apply_ty,
                    table_index: 0,
                });
                fx.op(I::LocalSet(r));
                // predicate must be a bool box, else trap (oracle: error)
                fx.op(I::LocalGet(r));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_BOOL));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                // if r's value != 0: out[8+4*k] = elem; k += 1
                fx.op(I::LocalGet(r));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::If(BlockType::Empty));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(k));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(elem));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(k));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(k));
                fx.op(I::End);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End); // loop
                fx.op(I::End); // block
                // out[4] = k
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(k));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(out));
            }
            "zip" => {
                // Pair two lists element-wise into a list of 2-tuples, stopping at
                // the shorter (oracle: x.into_iter().zip(b)). Non-list traps.
                nargs(2)?;
                let a = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(a));
                let b = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(b));
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n1 = fx.local(ValType::I32);
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n1));
                let n2 = fx.local(ValType::I32);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n2));
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(n1));
                fx.op(I::LocalGet(n2));
                fx.op(I::I32LtU);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                fx.op(I::LocalGet(n1));
                fx.op(I::Else);
                fx.op(I::LocalGet(n2));
                fx.op(I::End);
                fx.op(I::LocalSet(n));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(n));
                fx.op(I::I32Store(ma(4, 2)));
                let i = fx.local(ValType::I32);
                let tup = fx.local(ValType::I32);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // tup = [TAG_TUP, 2, a[8+4i], b[8+4i]]
                fx.op(I::I32Const(16));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(tup));
                fx.op(I::LocalGet(tup));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(tup));
                fx.op(I::I32Const(2));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(tup));
                fx.op(I::LocalGet(a));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Store(ma(8, 2)));
                fx.op(I::LocalGet(tup));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Store(ma(12, 2)));
                // out[8+4i] = tup
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(tup));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(out));
            }
            "apply" => {
                // apply(f, payload): call the closure value `f` with `payload` as
                // its argument bundle, via the boxed-closure convention
                // (call_indirect(env=f, payload, slot=f[4])) — the same seam
                // `map`/`fold`/`filter` use. The interpreter's `apply` binds the
                // payload per the function's arity (a single value to a 1-param
                // fn, a TAG_TUP bundle to an n-param fn), which is exactly what
                // the closure wrapper unpacks.
                nargs(2)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(fp));
                let pay = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(pay));
                let apply_ty = self.ty_idx(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
                fx.op(I::LocalGet(fp));
                fx.op(I::LocalGet(pay));
                fx.op(I::LocalGet(fp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::CallIndirect {
                    type_index: apply_ty,
                    table_index: 0,
                });
            }
            _ => unreachable!("builtin_higher_order routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Element access and growth: `get`/`put` (indexed read/replace),
    /// `push`, and `concat`.
    pub(crate) fn builtin_index(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "get" => {
                // Index a list OR tuple; out-of-range (idx >= len, unsigned so a
                // negative index also) traps, like the oracle's range error. A
                // record or other tag traps (the oracle errors on those too).
                nargs(2)?;
                let lst = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(lst));
                let idx = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I32WrapI64);
                fx.op(I::LocalSet(idx));
                let tg = fx.local(ValType::I32);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::LocalTee(tg));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Eq);
                fx.op(I::LocalGet(tg));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Eq);
                fx.op(I::I32Or);
                fx.op(I::I32Eqz);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(idx));
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::I32GeU);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(idx));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
            }
            "put" => {
                // Copy the list and overwrite element `idx` with `v`, returning
                // the new list; idx >= len traps (oracle range error). `put`
                // requires a list (not a tuple), matching the oracle's want_list.
                nargs(3)?;
                let lst = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(lst));
                let idx = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I32WrapI64);
                fx.op(I::LocalSet(idx));
                let v = fx.local(ValType::I32);
                self.expr(fx, items[2], false)?;
                fx.op(I::LocalSet(v));
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                fx.op(I::LocalGet(idx));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let sz = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(sz));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(sz));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(lst));
                fx.op(I::LocalGet(sz));
                fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(idx));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(v));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
            }
            "push" => {
                // Copy the list and append `v` at the end, returning the new list.
                nargs(2)?;
                let lst = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(lst));
                let v = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(v));
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(lst));
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(v));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
            }
            "concat" => {
                // Concatenate two lists into a fresh list (oracle: want_list ++
                // want_list). A non-list operand traps.
                nargs(2)?;
                let a = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(a));
                let b = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(b));
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n1 = fx.local(ValType::I32);
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n1));
                let n2 = fx.local(ValType::I32);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n2));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n1));
                fx.op(I::LocalGet(n2));
                fx.op(I::I32Add);
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(n1));
                fx.op(I::LocalGet(n2));
                fx.op(I::I32Add);
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(a));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(n1));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(n1));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(n2));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
                fx.op(I::LocalGet(out));
            }
            _ => unreachable!("builtin_index routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Search and partition over strings/lists: `contains`, `join`, and
    /// `split` (which delegates to `builtin_split`).
    pub(crate) fn builtin_search(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "join" => {
                // Concatenate the list's string elements with `sep` between them
                // (oracle: want_list of parts, want_str each, want_str sep).
                // `strcat2` tag-checks its operands, so a non-string part/sep
                // traps exactly where the oracle errors; the empty-list result
                // is "" but `sep` is still validated (oracle pops it first).
                nargs(2)?;
                let parts = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(parts));
                let sep = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(sep));
                fx.op(I::LocalGet(parts));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(sep));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_STR));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(parts));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                let acc = fx.local(ValType::I32);
                fx.op(I::I32Const(self.intern_str("") as i32));
                fx.op(I::LocalSet(acc));
                let i = fx.local(ValType::I32);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // acc = strcat2(acc, sep) when i > 0
                fx.op(I::LocalGet(i));
                fx.op(I::If(BlockType::Empty));
                fx.op(I::LocalGet(acc));
                fx.op(I::LocalGet(sep));
                fx.op(I::Call(self.h.strcat2));
                fx.op(I::LocalSet(acc));
                fx.op(I::End);
                // acc = strcat2(acc, parts[8+4i])
                fx.op(I::LocalGet(acc));
                fx.op(I::LocalGet(parts));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::Call(self.h.strcat2));
                fx.op(I::LocalSet(acc));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(acc));
            }
            "contains" => {
                // Substring test over UTF-8 bytes (oracle: s.contains(sub), both
                // strings). The empty substring is contained in every string,
                // matching Rust. Non-string operands trap (want_str).
                nargs(2)?;
                let s = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(s));
                let sub = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(sub));
                for str_box in [s, sub] {
                    fx.op(I::LocalGet(str_box));
                    fx.op(I::I32Load(ma(0, 2)));
                    fx.op(I::I32Const(TAG_STR));
                    fx.op(I::I32Ne);
                    fx.op(I::If(BlockType::Empty));
                    fx.op(I::Unreachable);
                    fx.op(I::End);
                }
                let slen = fx.local(ValType::I32);
                fx.op(I::LocalGet(s));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(slen));
                let sublen = fx.local(ValType::I32);
                fx.op(I::LocalGet(sub));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(sublen));
                let found = fx.local(ValType::I32);
                let matched = fx.local(ValType::I32);
                let i = fx.local(ValType::I32);
                let k = fx.local(ValType::I32);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(found));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty)); // outer
                fx.op(I::Loop(BlockType::Empty)); // iloop
                fx.op(I::LocalGet(found));
                fx.op(I::BrIf(1));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(sublen));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(slen));
                fx.op(I::I32GtU);
                fx.op(I::BrIf(1));
                fx.op(I::I32Const(1));
                fx.op(I::LocalSet(matched));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(k));
                fx.op(I::Block(BlockType::Empty)); // jbreak
                fx.op(I::Loop(BlockType::Empty)); // jloop
                fx.op(I::LocalGet(k));
                fx.op(I::LocalGet(sublen));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                fx.op(I::LocalGet(s));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(k));
                fx.op(I::I32Add);
                fx.op(I::I32Load8U(ma(0, 0)));
                fx.op(I::LocalGet(sub));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(k));
                fx.op(I::I32Add);
                fx.op(I::I32Load8U(ma(0, 0)));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(matched));
                fx.op(I::Br(2));
                fx.op(I::End);
                fx.op(I::LocalGet(k));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(k));
                fx.op(I::Br(0));
                fx.op(I::End); // jloop
                fx.op(I::End); // jbreak
                fx.op(I::LocalGet(matched));
                fx.op(I::If(BlockType::Empty));
                fx.op(I::I32Const(1));
                fx.op(I::LocalSet(found));
                fx.op(I::End);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End); // iloop
                fx.op(I::End); // outer
                fx.op(I::LocalGet(found));
                fx.op(I::Call(self.h.box_bool));
            }
            "split" => return self.builtin_split(fx, items),
            _ => unreachable!("builtin_search routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// `s.split(sep)` — factored out of `builtin_search` because its byte
    /// scanner is by far the largest single builtin body.
    pub(crate) fn builtin_split(&mut self, fx: &mut FnCtx, items: &[NodeId]) -> Result<(), String> {
        // s.split(sep) over UTF-8 bytes (oracle: want_str s, want_str
        // sep). Non-empty sep: scan left-to-right for non-overlapping
        // byte matches, emitting the run before each match and the final
        // tail (at most slen+1 pieces, so the list is over-allocated and
        // its length fixed at the end). Empty sep replicates Rust's
        // char-boundary split: a leading "", one piece per char, a
        // trailing "".
        if items.len() != 2 {
            return Err(format!("`split` expects 2 argument(s), got {}", items.len()));
        }
        let s = fx.local(ValType::I32);
        self.expr(fx, items[0], false)?;
        fx.op(I::LocalSet(s));
        let sep = fx.local(ValType::I32);
        self.expr(fx, items[1], false)?;
        fx.op(I::LocalSet(sep));
        for str_box in [s, sep] {
            fx.op(I::LocalGet(str_box));
            fx.op(I::I32Load(ma(0, 2)));
            fx.op(I::I32Const(TAG_STR));
            fx.op(I::I32Ne);
            fx.op(I::If(BlockType::Empty));
            fx.op(I::Unreachable);
            fx.op(I::End);
        }
        let slen = fx.local(ValType::I32);
        fx.op(I::LocalGet(s));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(slen));
        let seplen = fx.local(ValType::I32);
        fx.op(I::LocalGet(sep));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(seplen));
        let out = fx.local(ValType::I32);
        let oi = fx.local(ValType::I32);
        let start = fx.local(ValType::I32);
        let i = fx.local(ValType::I32);
        let k = fx.local(ValType::I32);
        let matched = fx.local(ValType::I32);
        let plen = fx.local(ValType::I32);
        let pbox = fx.local(ValType::I32);
        let j = fx.local(ValType::I32);
        let empty = self.intern_str("") as i32;
        fx.op(I::LocalGet(seplen));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        // ---- empty-sep path (Rust char-boundary split) ----
        // out over-allocated to slen+2 pieces; length fixed at the end.
        fx.op(I::LocalGet(slen));
        fx.op(I::I32Const(2));
        fx.op(I::I32Add);
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(out));
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        // out[8] = ""  (leading empty piece)
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(empty));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(oi));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i)); // byte cursor
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(slen));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalSet(start)); // char start
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i)); // consume lead byte
        // advance over continuation bytes (b & 0xC0 == 0x80)
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(slen));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(s));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(0, 0)));
        fx.op(I::I32Const(0xC0));
        fx.op(I::I32And);
        fx.op(I::I32Const(0x80));
        fx.op(I::I32Ne);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        // plen = i - start ; piece = substr(s, start, plen)
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(start));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(plen));
        emit_substr(self, fx, s, start, plen, pbox, j);
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(pbox));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        // trailing empty piece
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Const(empty));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
        fx.op(I::Else);
        // ---- non-empty-sep path ----
        fx.op(I::LocalGet(slen));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(out));
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(oi));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(start));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(seplen));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(slen));
        fx.op(I::I32GtU);
        fx.op(I::BrIf(1));
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(matched));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(k));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(k));
        fx.op(I::LocalGet(seplen));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(s));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(k));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(0, 0)));
        fx.op(I::LocalGet(sep));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(k));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(0, 0)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(matched));
        fx.op(I::Br(2));
        fx.op(I::End);
        fx.op(I::LocalGet(k));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(k));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(matched));
        fx.op(I::If(BlockType::Empty));
        // emit piece s[start..i]
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(start));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(plen));
        emit_substr(self, fx, s, start, plen, pbox, j);
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(pbox));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(seplen));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalSet(start));
        fx.op(I::Else);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::End);
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        // final tail piece s[start..slen]
        fx.op(I::LocalGet(slen));
        fx.op(I::LocalGet(start));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(plen));
        emit_substr(self, fx, s, start, plen, pbox, j);
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(pbox));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
        fx.op(I::End);
        // fix the list length to the actual piece count
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(out));
        Ok(())
    }

    /// String builtins: `str-cat`, `to-string`, `to-char`, and
    /// `upper`/`lower`.
    pub(crate) fn builtin_string(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "str-cat" => {
                if items.is_empty() {
                    let a = self.intern_str("");
                    fx.op(I::I32Const(a as i32));
                    return Ok(());
                }
                self.expr(fx, items[0], false)?;
                for &x in &items[1..] {
                    self.expr(fx, x, false)?;
                    fx.op(I::Call(self.h.strcat2));
                }
            }
            "to-string" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.to_str));
            }
            "to-char" => {
                // A char passes through; an int must be a Unicode scalar value
                // (traps otherwise, like the interpreter's range error). Anything
                // else traps in unbox_int, matching `to-char expects an int`.
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                let b = fx.local(ValType::I32);
                fx.op(I::LocalSet(b));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_CHAR));
                fx.op(I::I32Eq);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                fx.op(I::LocalGet(b));
                fx.op(I::Else);
                let n = fx.local(ValType::I64);
                fx.op(I::LocalGet(b));
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::LocalSet(n));
                // invalid if n > 0x10FFFF (unsigned, catches negatives) or a
                // surrogate (0xD800..=0xDFFF)
                fx.op(I::LocalGet(n));
                fx.op(I::I64Const(0x10FFFF));
                fx.op(I::I64GtU);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(n));
                fx.op(I::I64Const(0xD800));
                fx.op(I::I64GeU);
                fx.op(I::LocalGet(n));
                fx.op(I::I64Const(0xDFFF));
                fx.op(I::I64LeU);
                fx.op(I::I32And);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(n));
                self.box_char(fx);
                fx.op(I::End);
            }
            "upper" | "lower" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::I32Const(if name == "upper" { 1 } else { 0 }));
                fx.op(I::Call(self.h.case_h));
            }
            _ => unreachable!("builtin_string routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Variant constructors: `some`, `ok`, and `err`.
    pub(crate) fn builtin_variant(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        match name {
            "some" | "ok" | "err" => {
                // the argument(s) bundle into the variant payload, exactly as
                // the interpreter binds it. `ok()`/`err()` with no arguments
                // construct the payload-less case (4.2), like the interpreter.
                if items.is_empty() && name != "some" {
                    let addr = self.none_like_box(name);
                    fx.op(I::I32Const(addr as i32));
                    return Ok(());
                }
                self.var_box(fx, name, items)
            }
            _ => unreachable!("builtin_variant routed unexpected name `{name}`"),
        }
    }

    /// Mutable cell builtins: `cell-new`, `cell-get`, and `cell-set`.
    pub(crate) fn builtin_cell(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "cell-new" => {
                // A mutable cell holding one boxed value; its heap pointer is
                // its identity (interp: `Value::Cell(Rc<RefCell<Value>>)`).
                // In a resource/functor component a cell is resource state (a
                // `DefResource` `New`), so the cell AND its value live in the
                // persistent region and survive the per-call arena reset (5.1).
                nargs(1)?;
                let persist = self.has_persist();
                let v = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                if persist {
                    fx.op(I::Call(self.h.persist));
                }
                fx.op(I::LocalSet(v));
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(8));
                fx.op(I::Call(if persist {
                    self.h.persist_alloc
                } else {
                    self.h.alloc
                }));
                fx.op(I::LocalSet(p));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(TAG_CELL));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::LocalGet(v));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(p));
            }
            "cell-get" => {
                // Current value box, at cell offset 4.
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::I32Load(ma(4, 2)));
            }
            "cell-set" => {
                // Overwrite the cell's value box; return unit (interp parity).
                // In a resource/functor component the stored value is routed
                // through the persistent write barrier so it outlives the reset
                // (5.1); the cell itself was already allocated persistently.
                nargs(2)?;
                let persist = self.has_persist();
                let c = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(c));
                fx.op(I::LocalGet(c));
                self.expr(fx, items[1], false)?;
                if persist {
                    fx.op(I::Call(self.h.persist));
                }
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::I32Const(self.unit_addr() as i32));
            }
            _ => unreachable!("builtin_cell routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Compile-time form machinery used by macro bodies: `form-kind`,
    /// `rec-key`, `rec-val`, `gensym`, and `expand`.
    pub(crate) fn builtin_form(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "form-kind" => {
                nargs(1)?;
                return self.form_kind(fx, items[0]);
            }
            "rec-key" => {
                // First field's key as a payload-less variant: build
                // `[TAG_VAR, key-str-box, 0]` over the key box at rec offset 8.
                nargs(1)?;
                let rp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(rp));
                self.rec_guard(fx, rp);
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(12));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(p));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(TAG_VAR));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::LocalGet(rp));
                fx.op(I::I32Load(ma(8, 2)));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(0));
                fx.op(I::I32Store(ma(8, 2)));
                fx.op(I::LocalGet(p));
            }
            "rec-val" => {
                // First field's value box, at rec offset 12.
                nargs(1)?;
                let rp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(rp));
                self.rec_guard(fx, rp);
                fx.op(I::LocalGet(rp));
                fx.op(I::I32Load(ma(12, 2)));
            }
            "gensym" => {
                nargs(0)?;
                return self.gensym(fx);
            }
            "expand" => {
                nargs(1)?;
                match self.macro_expand_idx {
                    // Inside a macro component: one expansion step over the
                    // library's own macros (mirrors `builtins.rs` `expand`).
                    Some(idx) => {
                        self.expr(fx, items[0], false)?;
                        fx.op(I::Call(idx));
                    }
                    None => {
                        return Err("`expand` is only available inside a macro library \
                             (a file whose top level is DefMacros)"
                            .into());
                    }
                }
            }
            _ => unreachable!("builtin_form routed unexpected name `{name}`"),
        }
        Ok(())
    }
}


pub(crate) const BUILTINS: &[&str] = &[
    "eq",
    "not",
    "lt",
    "le",
    "gt",
    "ge",
    "add",
    "sub",
    "mul",
    "div",
    "rem",
    "neg",
    "min",
    "max",
    "abs",
    "len",
    "empty",
    "drop",
    "get",
    "put",
    "push",
    "concat",
    "reverse",
    "range",
    "zip",
    "split",
    "join",
    "contains",
    "apply",
    "head",
    "tail",
    "map",
    "fold",
    "filter",
    "str-cat",
    "upper",
    "lower",
    "to-string",
    "to-char",
    "to-u8",
    "to-u16",
    "to-u32",
    "to-u64",
    "to-s8",
    "to-s16",
    "to-s32",
    "to-s64",
    "to-f32",
    "to-f64",
    "some",
    "ok",
    "err",
    "cell-new",
    "cell-get",
    "cell-set",
    // compile-time form machinery (macro bodies)
    "form-kind",
    "rec-key",
    "rec-val",
    "gensym",
    "expand",
];
