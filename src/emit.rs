//! Phase 5: emit a core wasm module per file and wrap it into a component
//! (§9 read → expand → analyze → emit → componentize).
//!
//! v0 backend scope: enough of the language to compile the §1 demo and
//! similar programs. Values are boxed in linear memory (bump allocator, no
//! GC — leaks are fine for short-lived commands):
//!
//!   offset 0: tag i32     0=bool  1=int  2=str  3=list  4=dec
//!   bool: i32 value @4              int: i64 @8
//!   str:  i32 len @4, bytes @8      list: i32 len @4, i32 box ptrs @8
//!   dec:  f64 @8
//!
//! Every Wavelet value is an i32 pointer to a box. Internal functions take
//! one i32 per parameter and return an i32; tail calls use `return_call`.

use std::collections::HashMap;

use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportKind, ExportSection,
    Function, FunctionSection, GlobalSection, GlobalType, ImportSection, Instruction as I,
    MemArg, MemorySection, MemoryType, Module, TypeSection, ValType,
};

use crate::form::{Arena, Node, NodeId};
use crate::wit::{type_decl, FileInfo, FuncSig};

/// What the build step knows about a dependency component in the build set.
pub struct Dep {
    /// full package id with version, e.g. `demo:shout@0.1.0`
    pub package: String,
    pub funcs: Vec<FuncSig>,
    /// nested-package WIT text: `package demo:shout@0.1.0 { interface api {…} }`
    pub package_wit: String,
}

const SCRATCH: i32 = 0; // 0..16 reserved as canonical-ABI return area
const DATA_BASE: u32 = 16;
const TAG_BOOL: i32 = 0;
const TAG_INT: i32 = 1;
const TAG_STR: i32 = 2;
const TAG_LIST: i32 = 3;
const TAG_DEC: i32 = 4;

fn ma(offset: u64, align: u32) -> MemArg {
    MemArg { offset, align, memory_index: 0 }
}

// ---------------------------------------------------------------- WIT types

#[derive(Clone, Copy, PartialEq)]
enum WitTy {
    Bool,
    IntS, // s8/s16/s32 — i32 flat, sign-extended into the int box
    IntU, // u8/u16/u32
    S64,  // s64/u64 — i64 flat
    F64,
    Str,
}

fn wit_ty(s: &str) -> Result<WitTy, String> {
    Ok(match s {
        "bool" => WitTy::Bool,
        "s8" | "s16" | "s32" => WitTy::IntS,
        "u8" | "u16" | "u32" => WitTy::IntU,
        "s64" | "u64" => WitTy::S64,
        "f64" => WitTy::F64,
        "string" => WitTy::Str,
        other => return Err(format!("type `{other}` not supported by the wasm backend yet")),
    })
}

fn flat(ty: WitTy) -> &'static [ValType] {
    match ty {
        WitTy::Bool | WitTy::IntS | WitTy::IntU => &[ValType::I32],
        WitTy::S64 => &[ValType::I64],
        WitTy::F64 => &[ValType::F64],
        WitTy::Str => &[ValType::I32, ValType::I32],
    }
}

enum FlatRes {
    None,
    One(WitTy),
    Retptr, // flattened result > 1 value (string): pass/return a pointer
}

fn flat_result(sig: &FuncSig) -> Result<FlatRes, String> {
    match &sig.result {
        None => Ok(FlatRes::None),
        Some(t) => {
            let ty = wit_ty(t)?;
            if flat(ty).len() > 1 { Ok(FlatRes::Retptr) } else { Ok(FlatRes::One(ty)) }
        }
    }
}

// ------------------------------------------------------------ feature scan

#[derive(Default)]
struct Features {
    needs_stdout: bool,
    needs_env: bool,
    /// unique (alias, func) cross-component calls, in first-use order
    dep_calls: Vec<(String, String)>,
}

fn scan(arena: &Arena, id: NodeId, feats: &mut Features) {
    match arena.node(id) {
        Node::Call(head, payload) => {
            match arena.node(*head) {
                Node::Sym(s) if s == "print" || s == "println" => feats.needs_stdout = true,
                Node::Sym(s) if s == "args" => feats.needs_env = true,
                Node::Qsym(alias, name) => {
                    let key = (alias.clone(), name.clone());
                    if !feats.dep_calls.contains(&key) {
                        feats.dep_calls.push(key);
                    }
                }
                _ => {}
            }
            scan(arena, *head, feats);
            scan(arena, *payload, feats);
        }
        Node::Tup(xs) | Node::Lst(xs) => {
            for &x in xs {
                scan(arena, x, feats);
            }
        }
        Node::Rec(fields) => {
            for (_, v) in fields {
                scan(arena, *v, feats);
            }
        }
        _ => {}
    }
}

// ------------------------------------------------------- function building

struct FnCtx {
    instrs: Vec<I<'static>>,
    n_params: u32,
    extra_locals: Vec<ValType>,
    scopes: Vec<HashMap<String, u32>>,
}

impl FnCtx {
    fn new(n_params: u32) -> Self {
        FnCtx { instrs: Vec::new(), n_params, extra_locals: Vec::new(), scopes: vec![] }
    }
    fn local(&mut self, ty: ValType) -> u32 {
        let idx = self.n_params + self.extra_locals.len() as u32;
        self.extra_locals.push(ty);
        idx
    }
    fn op(&mut self, i: I<'static>) {
        self.instrs.push(i);
    }
    fn lookup(&self, name: &str) -> Option<u32> {
        for scope in self.scopes.iter().rev() {
            if let Some(&i) = scope.get(name) {
                return Some(i);
            }
        }
        None
    }
    fn finish(self) -> Function {
        let mut f = Function::new_with_locals_types(self.extra_locals);
        for i in &self.instrs {
            f.instruction(i);
        }
        f.instruction(&I::End);
        f
    }
}

// ------------------------------------------------------------- helper ids

struct Helpers {
    alloc: u32,
    realloc: u32,
    box_int: u32,
    box_bool: u32,
    box_dec: u32,
    box_str: u32,
    truthy: u32,
    unbox_int: u32,
    unbox_dec: u32,
    eq_raw: u32,
    len_raw: u32,
    head_h: u32,
    strcat2: u32,
    case_h: u32,
    print_str: Option<u32>,
    println_h: Option<u32>,
    get_args: Option<u32>,
}

// ---------------------------------------------------------------- emitter

pub fn emit_component(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    let mut module = emit_core_module(arena, roots, info, deps)?;
    let wit = synthesize_world_wit(arena, info, deps, &features_of(arena, info))?;

    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve
        .push_str("wavelet-synthesized.wit", &wit)
        .map_err(|e| format!("internal: synthesized WIT did not parse: {e:#}\n--- WIT ---\n{wit}"))?;
    let world = resolve
        .select_world(&[pkg], Some(&info.world))
        .map_err(|e| format!("internal: world selection failed: {e:#}"))?;
    wit_component::embed_component_metadata(
        &mut module,
        &resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )
    .map_err(|e| format!("embedding component metadata failed: {e:#}"))?;

    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&module)
        .map_err(|e| format!("componentizing failed: {e:#}"))?
        .encode()
        .map_err(|e| format!("component encoding failed: {e:#}"))
}

fn features_of(arena: &Arena, info: &FileInfo) -> Features {
    let mut feats = Features::default();
    for (_, (params, body)) in &info.defs {
        let _ = params;
        scan(arena, *body, &mut feats);
    }
    for (_, expr) in &info.value_defs {
        scan(arena, *expr, &mut feats);
    }
    feats
}

/// `("demo:shout@0.1.0", "api")` → `"demo:shout/api@0.1.0"`
fn versioned_iface(pkg: &str, iface: &str) -> String {
    match pkg.split_once('@') {
        Some((base, ver)) => format!("{base}/{iface}@{ver}"),
        None => format!("{pkg}/{iface}"),
    }
}

struct Emitter<'a> {
    arena: &'a Arena,
    info: &'a FileInfo,
    deps: &'a HashMap<String, Dep>,
    data: Vec<u8>, // segment contents, lives at DATA_BASE
    str_cache: HashMap<String, u32>,
    types: Vec<(Vec<ValType>, Vec<ValType>)>,
    imports: Vec<(String, String, u32)>, // module, field, type idx
    import_fn: HashMap<(String, String), u32>,
    h: Helpers,
    funcs: HashMap<String, (u32, Vec<String>)>, // internal defs
    bodies: Vec<(u32, Function)>,               // (type idx, body) for defined funcs
    false_addr: u32,
    true_addr: u32,
    nl_addr: u32,
}

impl<'a> Emitter<'a> {
    /// v0 has no record boxes; the unit value `{}` shares the static false box.
    fn unit_addr(&self) -> u32 {
        self.false_addr
    }

    fn ty_idx(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        if let Some(i) = self.types.iter().position(|t| t.0 == params && t.1 == results) {
            return i as u32;
        }
        self.types.push((params, results));
        (self.types.len() - 1) as u32
    }

    fn align8(&mut self) {
        while (DATA_BASE as usize + self.data.len()) % 8 != 0 {
            self.data.push(0);
        }
    }

    fn put_i32(&mut self, v: i32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    /// Intern a static string box; returns its address.
    fn intern_str(&mut self, s: &str) -> u32 {
        if let Some(&a) = self.str_cache.get(s) {
            return a;
        }
        self.align8();
        let addr = DATA_BASE + self.data.len() as u32;
        self.put_i32(TAG_STR);
        self.put_i32(s.len() as i32);
        self.data.extend_from_slice(s.as_bytes());
        self.str_cache.insert(s.to_string(), addr);
        addr
    }

    fn import_idx(&self, module: &str, field: &str) -> u32 {
        self.import_fn[&(module.to_string(), field.to_string())]
    }

    // -------------------------------------------------------- expressions

    fn expr(&mut self, fx: &mut FnCtx, id: NodeId, tail: bool) -> Result<(), String> {
        match self.arena.node(id).clone() {
            Node::Int(n) => {
                fx.op(I::I64Const(n));
                fx.op(I::Call(self.h.box_int));
            }
            Node::Dec(d) => {
                fx.op(I::F64Const(d.into()));
                fx.op(I::Call(self.h.box_dec));
            }
            Node::Bool(b) => {
                let a = if b { self.true_addr } else { self.false_addr };
                fx.op(I::I32Const(a as i32));
            }
            Node::Str(s) => {
                let a = self.intern_str(&s);
                fx.op(I::I32Const(a as i32));
            }
            Node::Char(_) => return Err("char values not supported by the wasm backend yet".into()),
            Node::Sym(name) => match fx.lookup(&name) {
                Some(idx) => fx.op(I::LocalGet(idx)),
                None => {
                    return Err(format!(
                        "`{name}` is not a local binding (first-class functions and \
                         module-level values are not supported by the wasm backend yet)"
                    ));
                }
            },
            Node::Call(head, payload) => return self.call(fx, head, payload, tail),
            Node::Tup(_) | Node::Lst(_) | Node::Rec(_) | Node::Flg(_) => {
                return Err("composite literals not supported by the wasm backend yet".into());
            }
            Node::Qsym(a, n) => {
                return Err(format!("`{a}/{n}` used as a value (only calls are supported)"));
            }
        }
        Ok(())
    }

    fn payload_items(&self, payload: NodeId) -> Vec<NodeId> {
        match self.arena.node(payload) {
            Node::Lst(xs) | Node::Tup(xs) => xs.clone(),
            _ => vec![payload],
        }
    }

    fn call(
        &mut self,
        fx: &mut FnCtx,
        head: NodeId,
        payload: NodeId,
        tail: bool,
    ) -> Result<(), String> {
        let head_node = self.arena.node(head).clone();
        match head_node {
            Node::Qsym(alias, fname) => self.dep_call(fx, &alias, &fname, payload),
            Node::Sym(name) => match name.as_str() {
                "if-MACRO" => self.if_form(fx, payload, tail),
                "do-MACRO" => self.do_form(fx, payload, tail),
                "let-MACRO" => self.let_form(fx, payload, tail),
                "the-MACRO" => {
                    let Node::Tup(items) = self.arena.node(payload) else {
                        return Err("malformed The".into());
                    };
                    let expr = items[1];
                    self.expr(fx, expr, tail)
                }
                "match-MACRO" | "fn-MACRO" | "quote-MACRO" | "quasi-MACRO" | "def-MACRO"
                | "def-macro-MACRO" => {
                    Err(format!("`{name}` not supported by the wasm backend yet"))
                }
                _ if BUILTINS.contains(&name.as_str()) => self.builtin(fx, &name, payload),
                _ => {
                    if self.funcs.contains_key(&name) {
                        self.internal_call(fx, &name, payload, tail)
                    } else {
                        Err(format!("unknown function `{name}` (wasm backend)"))
                    }
                }
            },
            _ => Err("call head must be a name (wasm backend)".into()),
        }
    }

    fn if_form(&mut self, fx: &mut FnCtx, payload: NodeId, tail: bool) -> Result<(), String> {
        let Node::Tup(items) = self.arena.node(payload).clone() else {
            return Err("malformed If".into());
        };
        let (c, t, e) = (items[0], items[1], items[2]);
        self.expr(fx, c, false)?;
        fx.op(I::Call(self.h.truthy));
        fx.op(I::If(BlockType::Result(ValType::I32)));
        self.expr(fx, t, tail)?;
        fx.op(I::Else);
        self.expr(fx, e, tail)?;
        fx.op(I::End);
        Ok(())
    }

    fn do_form(&mut self, fx: &mut FnCtx, payload: NodeId, tail: bool) -> Result<(), String> {
        let items = self.payload_items(payload);
        if items.is_empty() {
            fx.op(I::I32Const(self.unit_addr() as i32));
            return Ok(());
        }
        for &x in &items[..items.len() - 1] {
            self.expr(fx, x, false)?;
            fx.op(I::Drop);
        }
        self.expr(fx, items[items.len() - 1], tail)
    }

    fn let_form(&mut self, fx: &mut FnCtx, payload: NodeId, tail: bool) -> Result<(), String> {
        let Node::Tup(items) = self.arena.node(payload).clone() else {
            return Err("malformed Let".into());
        };
        let Node::Rec(fields) = self.arena.node(items[0]).clone() else {
            return Err("Let bindings must be a record".into());
        };
        fx.scopes.push(HashMap::new());
        for (k, v) in &fields {
            self.expr(fx, *v, false)?;
            let l = fx.local(ValType::I32);
            fx.op(I::LocalSet(l));
            fx.scopes.last_mut().unwrap().insert(k.clone(), l);
        }
        let r = self.expr(fx, items[1], tail);
        fx.scopes.pop();
        r
    }

    /// Mirror of the interpreter's §4.2 payload-binding rule, at compile time.
    fn bind_args(&self, payload: NodeId, params: &[String]) -> Result<Vec<NodeId>, String> {
        if let Node::Rec(fields) = self.arena.node(payload) {
            let mut keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
            let mut want: Vec<&str> = params.iter().map(|s| s.as_str()).collect();
            keys.sort();
            want.sort();
            if keys == want {
                let map: HashMap<&str, NodeId> =
                    fields.iter().map(|(k, v)| (k.as_str(), *v)).collect();
                return Ok(params.iter().map(|p| map[p.as_str()]).collect());
            }
        }
        if params.len() == 1 {
            return Ok(vec![payload]);
        }
        match self.arena.node(payload) {
            Node::Lst(xs) | Node::Tup(xs) if xs.len() == params.len() => Ok(xs.clone()),
            _ => Err(format!(
                "payload does not match parameters ({})",
                params.join(", ")
            )),
        }
    }

    fn internal_call(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        payload: NodeId,
        tail: bool,
    ) -> Result<(), String> {
        let (idx, params) = self.funcs[name].clone();
        let args = self.bind_args(payload, &params)?;
        for a in args {
            self.expr(fx, a, false)?;
        }
        fx.op(if tail { I::ReturnCall(idx) } else { I::Call(idx) });
        Ok(())
    }

    fn dep_call(
        &mut self,
        fx: &mut FnCtx,
        alias: &str,
        fname: &str,
        payload: NodeId,
    ) -> Result<(), String> {
        let imp = self
            .info
            .imports
            .iter()
            .find(|i| i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        let dep = self
            .deps
            .get(&imp.package)
            .ok_or(format!("dependency `{}` is not in the build set", imp.package))?;
        let sig = dep
            .funcs
            .iter()
            .find(|f| f.name == fname)
            .ok_or(format!("`{}` does not export `{fname}`", imp.package))?
            .clone();
        let iface = imp.path.rsplit('/').next().unwrap_or("api");
        let module = versioned_iface(&dep.package, iface);
        let fidx = self.import_idx(&module, fname);

        let param_names: Vec<String> = sig.params.iter().map(|(n, _)| n.clone()).collect();
        let args = self.bind_args(payload, &param_names)?;
        for (a, (_, t)) in args.iter().zip(&sig.params) {
            self.expr(fx, *a, false)?;
            self.lower(fx, wit_ty(t)?);
        }
        match flat_result(&sig)? {
            FlatRes::None => {
                fx.op(I::Call(fidx));
                fx.op(I::I32Const(self.unit_addr() as i32));
            }
            FlatRes::One(t) => {
                fx.op(I::Call(fidx));
                self.lift(fx, t);
            }
            FlatRes::Retptr => {
                fx.op(I::I32Const(SCRATCH));
                fx.op(I::Call(fidx));
                // result is a string: (ptr, len) written at the scratch area
                fx.op(I::I32Const(SCRATCH));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(SCRATCH));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::Call(self.h.box_str));
            }
        }
        Ok(())
    }

    /// box on stack → flat value(s) on stack
    fn lower(&mut self, fx: &mut FnCtx, ty: WitTy) {
        match ty {
            WitTy::Bool => fx.op(I::Call(self.h.truthy)),
            WitTy::IntS | WitTy::IntU => {
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I32WrapI64);
            }
            WitTy::S64 => fx.op(I::Call(self.h.unbox_int)),
            WitTy::F64 => fx.op(I::Call(self.h.unbox_dec)),
            WitTy::Str => {
                let t = fx.local(ValType::I32);
                fx.op(I::LocalTee(t));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(t));
                fx.op(I::I32Load(ma(4, 2)));
            }
        }
    }

    /// flat value on stack → box on stack (single-flat types only)
    fn lift(&mut self, fx: &mut FnCtx, ty: WitTy) {
        match ty {
            WitTy::Bool => fx.op(I::Call(self.h.box_bool)),
            WitTy::IntS => {
                fx.op(I::I64ExtendI32S);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::IntU => {
                fx.op(I::I64ExtendI32U);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::S64 => fx.op(I::Call(self.h.box_int)),
            WitTy::F64 => fx.op(I::Call(self.h.box_dec)),
            WitTy::Str => unreachable!("string is never a single flat value"),
        }
    }

    fn builtin(&mut self, fx: &mut FnCtx, name: &str, payload: NodeId) -> Result<(), String> {
        let items = self.payload_items(payload);
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!("`{name}` expects {want} argument(s), got {}", items.len()))
            }
        };
        match name {
            "eq" => {
                nargs(2)?;
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
                nargs(2)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.unbox_int));
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(match name {
                    "lt" => I::I64LtS,
                    "le" => I::I64LeS,
                    "gt" => I::I64GtS,
                    _ => I::I64GeS,
                });
                fx.op(I::Call(self.h.box_bool));
            }
            "add" | "sub" | "mul" | "div" | "rem" => {
                if items.is_empty() {
                    return Err(format!("`{name}` needs at least one argument"));
                }
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.unbox_int));
                for &x in &items[1..] {
                    self.expr(fx, x, false)?;
                    fx.op(I::Call(self.h.unbox_int));
                    fx.op(match name {
                        "add" => I::I64Add,
                        "sub" => I::I64Sub,
                        "mul" => I::I64Mul,
                        "div" => I::I64DivS,
                        _ => I::I64RemS,
                    });
                }
                fx.op(I::Call(self.h.box_int));
            }
            "neg" => {
                nargs(1)?;
                fx.op(I::I64Const(0));
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I64Sub);
                fx.op(I::Call(self.h.box_int));
            }
            "len" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.len_raw));
                fx.op(I::I64ExtendI32U);
                fx.op(I::Call(self.h.box_int));
            }
            "head" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.head_h));
            }
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
            "upper" | "lower" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::I32Const(if name == "upper" { 1 } else { 0 }));
                fx.op(I::Call(self.h.case_h));
            }
            "print" | "println" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                let h = if name == "print" { self.h.print_str } else { self.h.println_h };
                fx.op(I::Call(h.expect("print helpers emitted when used")));
                fx.op(I::I32Const(self.unit_addr() as i32));
            }
            "args" => {
                nargs(0)?;
                fx.op(I::Call(self.h.get_args.expect("args helper emitted when used")));
            }
            other => return Err(format!("builtin `{other}` not supported by the wasm backend yet")),
        }
        Ok(())
    }
}

const BUILTINS: &[&str] = &[
    "eq", "not", "lt", "le", "gt", "ge", "add", "sub", "mul", "div", "rem", "neg", "len",
    "head", "str-cat", "upper", "lower", "print", "println", "args",
];

// --------------------------------------------------------- helper bodies

fn emit_core_module(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    let feats = features_of(arena, info);
    let is_command = info.target.as_deref() == Some("wasi:cli/command");

    let mut em = Emitter {
        arena,
        info,
        deps,
        data: Vec::new(),
        str_cache: HashMap::new(),
        types: Vec::new(),
        imports: Vec::new(),
        import_fn: HashMap::new(),
        h: Helpers {
            alloc: 0,
            realloc: 0,
            box_int: 0,
            box_bool: 0,
            box_dec: 0,
            box_str: 0,
            truthy: 0,
            unbox_int: 0,
            unbox_dec: 0,
            eq_raw: 0,
            len_raw: 0,
            head_h: 0,
            strcat2: 0,
            case_h: 0,
            print_str: None,
            println_h: None,
            get_args: None,
        },
        funcs: HashMap::new(),
        bodies: Vec::new(),
        false_addr: 0,
        true_addr: 0,
        nl_addr: 0,
    };

    // static boxes: false @16, true @24, "\n" box
    em.false_addr = DATA_BASE;
    em.put_i32(TAG_BOOL);
    em.put_i32(0);
    em.true_addr = DATA_BASE + 8;
    em.put_i32(TAG_BOOL);
    em.put_i32(1);
    em.nl_addr = em.intern_str("\n");

    // ---- imports (function index space starts here)
    let mut n_imports = 0u32;
    let mut add_import = |em: &mut Emitter, module: &str, field: &str, p: Vec<ValType>, r: Vec<ValType>| {
        let t = em.ty_idx(p, r);
        em.imports.push((module.to_string(), field.to_string(), t));
        em.import_fn
            .insert((module.to_string(), field.to_string()), n_imports);
        n_imports += 1;
    };

    use ValType::{F64, I32, I64};
    let _ = (I64, F64);
    if feats.needs_stdout {
        add_import(&mut em, "wasi:cli/stdout@0.2.0", "get-stdout", vec![], vec![I32]);
        add_import(
            &mut em,
            "wasi:io/streams@0.2.0",
            "[method]output-stream.blocking-write-and-flush",
            vec![I32, I32, I32, I32],
            vec![],
        );
    }
    if feats.needs_env {
        add_import(&mut em, "wasi:cli/environment@0.2.0", "get-arguments", vec![I32], vec![]);
    }
    for (alias, fname) in &feats.dep_calls {
        let imp = info
            .imports
            .iter()
            .find(|i| &i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        let dep = deps
            .get(&imp.package)
            .ok_or(format!("dependency `{}` is not in the build set", imp.package))?;
        let sig = dep
            .funcs
            .iter()
            .find(|f| &f.name == fname)
            .ok_or(format!("`{}` does not export `{fname}`", imp.package))?;
        let mut p = Vec::new();
        for (_, t) in &sig.params {
            p.extend_from_slice(flat(wit_ty(t)?));
        }
        let r = match flat_result(sig)? {
            FlatRes::None => vec![],
            FlatRes::One(t) => flat(t).to_vec(),
            FlatRes::Retptr => {
                p.push(I32);
                vec![]
            }
        };
        let iface = imp.path.rsplit('/').next().unwrap_or("api");
        let module = versioned_iface(&dep.package, iface);
        add_import(&mut em, &module, fname, p, r);
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
    em.h.unbox_dec = take();
    em.h.eq_raw = take();
    em.h.len_raw = take();
    em.h.head_h = take();
    em.h.strcat2 = take();
    em.h.case_h = take();
    if feats.needs_stdout {
        em.h.print_str = Some(take());
        em.h.println_h = Some(take());
    }
    if feats.needs_env {
        em.h.get_args = Some(take());
    }

    // ---- assign internal function indices (file order)
    let mut internal_order: Vec<String> = Vec::new();
    for &root in roots {
        if let Node::Call(h, p) = arena.node(root) {
            if matches!(arena.node(*h), Node::Sym(s) if s == "def-MACRO") {
                if let Node::Tup(items) = arena.node(*p) {
                    if let Node::Sym(name) = arena.node(items[0]) {
                        if info.defs.contains_key(name) && !internal_order.contains(name) {
                            internal_order.push(name.clone());
                        }
                    }
                }
            }
        }
    }
    if !info.value_defs.is_empty() {
        return Err(format!(
            "module-level value `Def {}` not supported by the wasm backend yet",
            info.value_defs[0].0
        ));
    }
    for name in &internal_order {
        let (params_id, _) = info.defs[name];
        let params = param_names(arena, params_id)?;
        em.funcs.insert(name.clone(), (take(), params));
    }

    // ---- helper bodies (order must match index assignment above)
    emit_helpers(&mut em, &feats)?;

    // ---- internal function bodies
    for name in &internal_order {
        let (_, body) = info.defs[name];
        let params = em.funcs[name].1.clone();
        let n = params.len();
        let mut fx = FnCtx::new(n as u32);
        let mut scope = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            scope.insert(p.clone(), i as u32);
        }
        fx.scopes.push(scope);
        em.expr(&mut fx, body, true)
            .map_err(|e| format!("in `{name}`: {e}"))?;
        let t = em.ty_idx(vec![I32; n], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- export wrappers
    let own_iface = versioned_iface(&info.package, "api");
    let mut exports: Vec<(String, u32)> = Vec::new(); // (export name, fn idx)
    for sig in &info.exports {
        let (fidx, _) = *em
            .funcs
            .get(&sig.name)
            .ok_or(format!("export `{}` has no Def Fn", sig.name))?;
        if is_command && sig.name == "run" {
            // wasi:cli/run@0.2.0#run: func() -> result
            let mut fx = FnCtx::new(0);
            fx.op(I::Call(fidx));
            fx.op(I::Drop);
            fx.op(I::I32Const(0)); // ok
            let t = em.ty_idx(vec![], vec![I32]);
            em.bodies.push((t, fx.finish()));
            exports.push(("wasi:cli/run@0.2.0#run".to_string(), take()));
            continue;
        }
        let mut fparams = Vec::new();
        let mut lifted: Vec<(WitTy, u32)> = Vec::new(); // (ty, first flat local)
        for (_, t) in &sig.params {
            let ty = wit_ty(t)?;
            lifted.push((ty, fparams.len() as u32));
            fparams.extend_from_slice(flat(ty));
        }
        let mut fx = FnCtx::new(fparams.len() as u32);
        for (ty, base) in &lifted {
            match ty {
                WitTy::Str => {
                    fx.op(I::LocalGet(*base));
                    fx.op(I::LocalGet(*base + 1));
                    fx.op(I::Call(em.h.box_str));
                }
                _ => {
                    fx.op(I::LocalGet(*base));
                    em.lift(&mut fx, *ty);
                }
            }
        }
        fx.op(I::Call(fidx));
        let fresults = match flat_result(sig)? {
            FlatRes::None => {
                fx.op(I::Drop);
                vec![]
            }
            FlatRes::One(t) => {
                em.lower(&mut fx, t);
                flat(t).to_vec()
            }
            FlatRes::Retptr => {
                // callee-owned return area holding (ptr, len)
                let b = fx.local(I32);
                let area = fx.local(I32);
                fx.op(I::LocalSet(b));
                fx.op(I::I32Const(8));
                fx.op(I::Call(em.h.alloc));
                fx.op(I::LocalTee(area));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(area));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(area));
                vec![I32]
            }
        };
        let t = em.ty_idx(fparams, fresults);
        em.bodies.push((t, fx.finish()));
        exports.push((format!("{own_iface}#{}", sig.name), take()));
    }

    // ---- assemble
    let heap_base = {
        em.align8();
        DATA_BASE + em.data.len() as u32
    };
    let pages = (heap_base as u64 >> 16) + 1;

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

    let mut fs = FunctionSection::new();
    for (t, _) in &em.bodies {
        fs.function(*t);
    }
    module.section(&fs);

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
    gs.global(
        GlobalType { val_type: I32, mutable: true, shared: false },
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

    let mut cs = CodeSection::new();
    for (_, f) in &em.bodies {
        cs.function(f);
    }
    module.section(&cs);

    let mut ds = DataSection::new();
    ds.active(0, &ConstExpr::i32_const(DATA_BASE as i32), em.data.iter().copied());
    module.section(&ds);

    Ok(module.finish())
}

fn param_names(arena: &Arena, params_id: NodeId) -> Result<Vec<String>, String> {
    match arena.node(params_id) {
        Node::Flg(names) => Ok(names.clone()),
        Node::Rec(fields) => Ok(fields.iter().map(|(k, _)| k.clone()).collect()),
        _ => Err("malformed Fn parameters".into()),
    }
}

fn emit_helpers(em: &mut Emitter, feats: &Features) -> Result<(), String> {
    use ValType::{F64, I32, I64};

    // alloc(n) -> ptr   [locals: r=1, end=2]
    {
        let mut fx = FnCtx::new(1);
        let r = fx.local(I32);
        let end = fx.local(I32);
        fx.op(I::GlobalGet(0));
        fx.op(I::LocalSet(r));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(7));
        fx.op(I::I32Add);
        fx.op(I::I32Const(-8));
        fx.op(I::I32And);
        fx.op(I::LocalSet(0));
        fx.op(I::LocalGet(r));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(end));
        fx.op(I::LocalGet(end));
        fx.op(I::MemorySize(0));
        fx.op(I::I32Const(16));
        fx.op(I::I32Shl);
        fx.op(I::I32GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(end));
        fx.op(I::MemorySize(0));
        fx.op(I::I32Const(16));
        fx.op(I::I32Shl);
        fx.op(I::I32Sub);
        fx.op(I::I32Const(0xffff));
        fx.op(I::I32Add);
        fx.op(I::I32Const(16));
        fx.op(I::I32ShrU);
        fx.op(I::MemoryGrow(0));
        fx.op(I::I32Const(-1));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(end));
        fx.op(I::GlobalSet(0));
        fx.op(I::LocalGet(r));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // cabi_realloc(old, old_size, align, new_size) -> ptr
    {
        let mut fx = FnCtx::new(4);
        let p = fx.local(I32);
        fx.op(I::LocalGet(3));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(1));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
        fx.op(I::End);
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32, I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_int(i64) -> ptr
    {
        let mut fx = FnCtx::new(1);
        let p = fx.local(I32);
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Store(ma(8, 3)));
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I64], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_bool(i32) -> ptr (static boxes)
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(em.true_addr as i32));
        fx.op(I::Else);
        fx.op(I::I32Const(em.false_addr as i32));
        fx.op(I::End);
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_dec(f64) -> ptr
    {
        let mut fx = FnCtx::new(1);
        let p = fx.local(I32);
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(0));
        fx.op(I::F64Store(ma(8, 3)));
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![F64], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_str(ptr, len) -> box
    {
        let mut fx = FnCtx::new(2);
        let p = fx.local(I32);
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // truthy(box) -> i32
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(1));
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::I32Ne);
        fx.op(I::End);
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // unbox_int(box) -> i64 (traps unless tag int)
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        let t = em.ty_idx(vec![I32], vec![I64]);
        em.bodies.push((t, fx.finish()));
    }

    // unbox_dec(box) -> f64
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::F64Load(ma(8, 3)));
        let t = em.ty_idx(vec![I32], vec![F64]);
        em.bodies.push((t, fx.finish()));
    }

    // eq_raw(a, b) -> i32   [locals: ta=2, la=3, i=4]
    {
        let mut fx = FnCtx::new(2);
        let ta = fx.local(I32);
        let la = fx.local(I32);
        let i = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalTee(ta));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        // bool
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // int
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // dec
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::F64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::F64Load(ma(8, 3)));
        fx.op(I::F64Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // str
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalTee(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(la));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        // lists & anything else: identity
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Eq);
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // len_raw(box) -> i32 (str or list)
    {
        let mut fx = FnCtx::new(1);
        let tg = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalTee(tg));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // head_h(list box) -> box
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(8, 2)));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // strcat2(a, b) -> box   [locals la, lb, p]
    {
        let mut fx = FnCtx::new(2);
        let la = fx.local(I32);
        let lb = fx.local(I32);
        let p = fx.local(I32);
        for arg in [0u32, 1u32] {
            fx.op(I::LocalGet(arg));
            fx.op(I::I32Load(ma(0, 2)));
            fx.op(I::I32Const(TAG_STR));
            fx.op(I::I32Ne);
            fx.op(I::If(BlockType::Empty));
            fx.op(I::Unreachable);
            fx.op(I::End);
        }
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(lb));
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(la));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(lb));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(lb));
        fx.op(I::I32Add);
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(la));
        fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(la));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(1));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(lb));
        fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // case_h(s, up) -> box   [locals l, p, i, c]
    {
        let mut fx = FnCtx::new(2);
        let l = fx.local(I32);
        let p = fx.local(I32);
        let i = fx.local(I32);
        let c = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(l));
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(l));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(l));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(l));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::LocalSet(c));
        fx.op(I::LocalGet(1));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'a' as i32));
        fx.op(I::I32GeU);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'z' as i32));
        fx.op(I::I32LeU);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(32));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(c));
        fx.op(I::End);
        fx.op(I::Else);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'A' as i32));
        fx.op(I::I32GeU);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'Z' as i32));
        fx.op(I::I32LeU);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(32));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(c));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Store8(ma(8, 0)));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    if feats.needs_stdout {
        let get_stdout = em.import_idx("wasi:cli/stdout@0.2.0", "get-stdout");
        let write = em.import_idx(
            "wasi:io/streams@0.2.0",
            "[method]output-stream.blocking-write-and-flush",
        );
        // print_str(box)
        {
            let mut fx = FnCtx::new(1);
            fx.op(I::Call(get_stdout));
            fx.op(I::LocalGet(0));
            fx.op(I::I32Const(8));
            fx.op(I::I32Add);
            fx.op(I::LocalGet(0));
            fx.op(I::I32Load(ma(4, 2)));
            fx.op(I::I32Const(SCRATCH));
            fx.op(I::Call(write));
            let t = em.ty_idx(vec![I32], vec![]);
            em.bodies.push((t, fx.finish()));
        }
        // println_h(box)
        {
            let mut fx = FnCtx::new(1);
            fx.op(I::LocalGet(0));
            fx.op(I::Call(em.h.print_str.unwrap()));
            fx.op(I::I32Const(em.nl_addr as i32));
            fx.op(I::Call(em.h.print_str.unwrap()));
            let t = em.ty_idx(vec![I32], vec![]);
            em.bodies.push((t, fx.finish()));
        }
    }

    if feats.needs_env {
        let get_arguments = em.import_idx("wasi:cli/environment@0.2.0", "get-arguments");
        // get_args() -> list box, dropping argv[0]
        // locals: base, n, m, lst, i, tmp, bx
        let mut fx = FnCtx::new(0);
        let base = fx.local(I32);
        let n = fx.local(I32);
        let m = fx.local(I32);
        let lst = fx.local(I32);
        let i = fx.local(I32);
        let tmp = fx.local(I32);
        let bx = fx.local(I32);
        fx.op(I::I32Const(SCRATCH));
        fx.op(I::Call(get_arguments));
        fx.op(I::I32Const(SCRATCH));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(base));
        fx.op(I::I32Const(SCRATCH));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(n));
        fx.op(I::LocalGet(n));
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::LocalGet(n));
        fx.op(I::I32Const(1));
        fx.op(I::I32Sub);
        fx.op(I::Else);
        fx.op(I::I32Const(0));
        fx.op(I::End);
        fx.op(I::LocalSet(m));
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(2));
        fx.op(I::I32Shl);
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(lst));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(lst));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // tmp = base + 8*(i+1)
        fx.op(I::LocalGet(base));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::I32Const(3));
        fx.op(I::I32Shl);
        fx.op(I::I32Add);
        fx.op(I::LocalSet(tmp));
        fx.op(I::LocalGet(tmp));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalGet(tmp));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::Call(em.h.box_str));
        fx.op(I::LocalSet(bx));
        fx.op(I::LocalGet(lst));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(2));
        fx.op(I::I32Shl);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(bx));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(lst));
        let t = em.ty_idx(vec![], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    Ok(())
}

// ----------------------------------------------------------- WIT synthesis

const WASI_PACKAGES: &str = r#"
package wasi:io@0.2.0 {
  interface error {
    resource error;
  }
  interface streams {
    use error.{error};
    variant stream-error {
      last-operation-failed(error),
      closed,
    }
    resource output-stream {
      blocking-write-and-flush: func(contents: list<u8>) -> result<_, stream-error>;
    }
  }
}
package wasi:cli@0.2.0 {
  interface stdout {
    use wasi:io/streams@0.2.0.{output-stream};
    get-stdout: func() -> output-stream;
  }
  interface environment {
    get-arguments: func() -> list<string>;
  }
  interface run {
    run: func() -> result;
  }
}
"#;

/// Render a dependency's nested-package WIT from its parsed surface.
pub fn dep_package_wit(arena: &Arena, info: &FileInfo) -> Result<String, String> {
    let mut out = format!("package {} {{\n  interface api {{\n", info.package);
    for (name, ty) in &info.types {
        out.push_str(&format!("    {};\n", type_decl(arena, name, *ty)?));
    }
    for sig in &info.exports {
        out.push_str(&format!("    {}\n", sig.to_wit()));
    }
    out.push_str("  }\n}\n");
    Ok(out)
}

fn synthesize_world_wit(
    arena: &Arena,
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
    feats: &Features,
) -> Result<String, String> {
    let is_command = info.target.as_deref() == Some("wasi:cli/command");
    let mut out = format!("package {};\n\n", info.package);

    let api_exports: Vec<&FuncSig> = info
        .exports
        .iter()
        .filter(|s| !(is_command && s.name == "run"))
        .collect();
    if !api_exports.is_empty() || !info.types.is_empty() {
        out.push_str("interface api {\n");
        for (name, ty) in &info.types {
            out.push_str(&format!("  {};\n", type_decl(arena, name, *ty)?));
        }
        for sig in &api_exports {
            out.push_str(&format!("  {}\n", sig.to_wit()));
        }
        out.push_str("}\n\n");
    }

    out.push_str(&format!("world {} {{\n", info.world));
    if feats.needs_stdout {
        out.push_str("  import wasi:cli/stdout@0.2.0;\n");
    }
    if feats.needs_env {
        out.push_str("  import wasi:cli/environment@0.2.0;\n");
    }
    for imp in &info.imports {
        let dep = deps
            .get(&imp.package)
            .ok_or(format!("dependency `{}` is not in the build set", imp.package))?;
        let iface = imp.path.rsplit('/').next().unwrap_or("api");
        out.push_str(&format!("  import {};\n", versioned_iface(&dep.package, iface)));
    }
    if !api_exports.is_empty() || !info.types.is_empty() {
        out.push_str("  export api;\n");
    }
    if is_command {
        out.push_str("  export wasi:cli/run@0.2.0;\n");
    }
    out.push_str("}\n");

    if feats.needs_stdout || feats.needs_env || is_command {
        out.push_str(WASI_PACKAGES);
    }
    for dep in deps.values() {
        out.push_str(&dep.package_wit);
    }
    Ok(out)
}
