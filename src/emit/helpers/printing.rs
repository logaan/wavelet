use super::*;
use ValType::{F64, I32, I64};

pub(super) fn emit_string(em: &mut Emitter) {
    let true_s = em.intern_str("true");
    let false_s = em.intern_str("false");
    // interned punctuation for the compound-value printers
    let str_lb = em.intern_str("[");
    let str_rb = em.intern_str("]");
    let str_lp = em.intern_str("(");
    let str_rp = em.intern_str(")");
    let str_lc = em.intern_str("{");
    let str_rc = em.intern_str("}");
    let str_comma = em.intern_str(", ");
    let str_colon = em.intern_str(": ");
    let str_cell = em.intern_str("cell(");
    let nan_s = em.intern_str("nan");
    let inf_s = em.intern_str("inf");
    let ninf_s = em.intern_str("-inf");
    let mut fx = FnCtx::new(1);
    let tag = fx.local(I32);
    let n = fx.local(I64);
    let neg = fx.local(I32);
    let buf = fx.local(I32);
    let i = fx.local(I32);
    // extra locals for the string-quoting branch
    let s_src = fx.local(I32);
    let s_len = fx.local(I32);
    let s_out = fx.local(I32);
    let s_oi = fx.local(I32);
    let s_ci = fx.local(I32);
    let s_byte = fx.local(I32);
    // extra locals for the compound-value branches
    let c_n = fx.local(I32);
    let c_i = fx.local(I32);
    let c_acc = fx.local(I32);
    let c_base = fx.local(I32);
    let c_elem = fx.local(I32);
    let c_key = fx.local(I32);
    let c_val = fx.local(I32);
    // extra locals for the float and char branches (which reuse `buf`
    // as their output buffer and `i` as its write index)
    let d_xf = fx.local(F64);
    let d_e = fx.local(I32);
    let d_k = fx.local(I32);
    let d_last = fx.local(I32);
    let d_t = fx.local(I32);
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::LocalSet(tag));
    // out[s_oi] = <const byte>; s_oi += 1
    let put_c = |fx: &mut FnCtx, out: u32, oi: u32, b: i32| {
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Add);
        fx.op(I::I32Const(b));
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
    };
    // string: quote + escape to match print_value's `{s:?}`. Escapes
    // `"` `\` `\n` `\t` `\r`; other bytes (incl. UTF-8 continuation) pass
    // through. Rust escapes other control/non-printable codepoints too, so
    // strings containing them still diverge from the oracle (kept SKIP);
    // the common printable cases now agree.
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_STR));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    // s_len = box@4 ; s_src = box+8
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(s_len));
    fx.op(I::LocalGet(0));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(s_src));
    // s_out = alloc(s_len*2 + 2)  (worst case: every byte -> 2 bytes, + 2 quotes)
    fx.op(I::LocalGet(s_len));
    fx.op(I::I32Const(6));
    fx.op(I::I32Mul);
    fx.op(I::I32Const(2));
    fx.op(I::I32Add);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(s_out));
    // out[0] = '"' ; s_oi = 1 ; s_ci = 0
    fx.op(I::LocalGet(s_out));
    fx.op(I::I32Const(b'"' as i32));
    fx.op(I::I32Store8(ma(0, 0)));
    fx.op(I::I32Const(1));
    fx.op(I::LocalSet(s_oi));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(s_ci));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    // if s_ci >= s_len break
    fx.op(I::LocalGet(s_ci));
    fx.op(I::LocalGet(s_len));
    fx.op(I::I32GeS);
    fx.op(I::BrIf(1));
    // s_byte = load8(s_src + s_ci)
    fx.op(I::LocalGet(s_src));
    fx.op(I::LocalGet(s_ci));
    fx.op(I::I32Add);
    fx.op(I::I32Load8U(ma(0, 0)));
    fx.op(I::LocalSet(s_byte));
    // escape ladder
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(b'"' as i32));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    put_c(&mut fx, s_out, s_oi, b'\\' as i32);
    put_c(&mut fx, s_out, s_oi, b'"' as i32);
    fx.op(I::Else);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(b'\\' as i32));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    put_c(&mut fx, s_out, s_oi, b'\\' as i32);
    put_c(&mut fx, s_out, s_oi, b'\\' as i32);
    fx.op(I::Else);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(b'\n' as i32));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    put_c(&mut fx, s_out, s_oi, b'\\' as i32);
    put_c(&mut fx, s_out, s_oi, b'n' as i32);
    fx.op(I::Else);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(b'\t' as i32));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    put_c(&mut fx, s_out, s_oi, b'\\' as i32);
    put_c(&mut fx, s_out, s_oi, b't' as i32);
    fx.op(I::Else);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(b'\r' as i32));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    put_c(&mut fx, s_out, s_oi, b'\\' as i32);
    put_c(&mut fx, s_out, s_oi, b'r' as i32);
    fx.op(I::Else);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(32));
    fx.op(I::I32LtU);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(127));
    fx.op(I::I32Eq);
    fx.op(I::I32Or);
    fx.op(I::If(BlockType::Empty));
    put_c(&mut fx, s_out, s_oi, b'\\' as i32);
    put_c(&mut fx, s_out, s_oi, b'u' as i32);
    put_c(&mut fx, s_out, s_oi, b'{' as i32);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(16));
    fx.op(I::I32GeU);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(s_out));
    fx.op(I::LocalGet(s_oi));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(4));
    fx.op(I::I32ShrU);
    fx.op(I::I32Const(48));
    fx.op(I::I32Add);
    fx.op(I::I32Store8(ma(0, 0)));
    fx.op(I::LocalGet(s_oi));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(s_oi));
    fx.op(I::End);
    fx.op(I::LocalGet(s_out));
    fx.op(I::LocalGet(s_oi));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(15));
    fx.op(I::I32And);
    fx.op(I::I32Const(10));
    fx.op(I::I32LtU);
    fx.op(I::If(BlockType::Result(ValType::I32)));
    fx.op(I::I32Const(48));
    fx.op(I::Else);
    fx.op(I::I32Const(87));
    fx.op(I::End);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Const(15));
    fx.op(I::I32And);
    fx.op(I::I32Add);
    fx.op(I::I32Store8(ma(0, 0)));
    fx.op(I::LocalGet(s_oi));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(s_oi));
    put_c(&mut fx, s_out, s_oi, b'}' as i32);
    fx.op(I::Else);
    // default: copy the byte verbatim
    fx.op(I::LocalGet(s_out));
    fx.op(I::LocalGet(s_oi));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(s_byte));
    fx.op(I::I32Store8(ma(0, 0)));
    fx.op(I::LocalGet(s_oi));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(s_oi));
    fx.op(I::End);
    fx.op(I::End); // \r
    fx.op(I::End); // \t
    fx.op(I::End); // \n
    fx.op(I::End); // backslash
    fx.op(I::End); // quote
    // s_ci += 1 ; continue
    fx.op(I::LocalGet(s_ci));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(s_ci));
    fx.op(I::Br(0));
    fx.op(I::End); // loop
    fx.op(I::End); // block
    // closing quote, then box
    put_c(&mut fx, s_out, s_oi, b'"' as i32);
    fx.op(I::LocalGet(s_out));
    fx.op(I::LocalGet(s_oi));
    fx.op(I::Call(em.h.box_str));
    fx.op(I::Return);
    fx.op(I::End);
    // bool: static "true"/"false"
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_BOOL));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(true_s as i32));
    fx.op(I::Return);
    fx.op(I::End);
    fx.op(I::I32Const(false_s as i32));
    fx.op(I::Return);
    fx.op(I::End);
    // ---- compound values: recurse via to_str, matching print_value ----
    let seq_strcat2 = em.h.strcat2;
    let seq_to_str = em.h.to_str;
    // list: [e0, e1, ...]
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    to_str_seq(
        &mut fx,
        0,
        c_n,
        c_i,
        c_acc,
        c_base,
        c_elem,
        str_lb,
        str_rb,
        str_comma,
        4,
        seq_strcat2,
        seq_to_str,
        true,
    );
    fx.op(I::End);
    // tuple: (e0, e1, ...)
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_TUP));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    to_str_seq(
        &mut fx,
        0,
        c_n,
        c_i,
        c_acc,
        c_base,
        c_elem,
        str_lp,
        str_rp,
        str_comma,
        4,
        seq_strcat2,
        seq_to_str,
        true,
    );
    fx.op(I::End);
    // flags: {a, b, ...} (names are str boxes, appended verbatim)
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_FLG));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    to_str_seq(
        &mut fx,
        0,
        c_n,
        c_i,
        c_acc,
        c_base,
        c_elem,
        str_lc,
        str_rc,
        str_comma,
        4,
        seq_strcat2,
        seq_to_str,
        false,
    );
    fx.op(I::End);
    // record: {k0: v0, k1: v1, ...}
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_REC));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(str_lc as i32));
    fx.op(I::LocalSet(c_acc));
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(c_n));
    fx.op(I::LocalGet(0));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(c_base));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(c_i));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(c_i));
    fx.op(I::LocalGet(c_n));
    fx.op(I::I32GeS);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(c_i));
    fx.op(I::I32Const(0));
    fx.op(I::I32GtS);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(c_acc));
    fx.op(I::I32Const(str_comma as i32));
    fx.op(I::Call(seq_strcat2));
    fx.op(I::LocalSet(c_acc));
    fx.op(I::End);
    // key = load(base + 8*i)  (a str box, appended verbatim)
    fx.op(I::LocalGet(c_base));
    fx.op(I::LocalGet(c_i));
    fx.op(I::I32Const(8));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::LocalSet(c_key));
    // val = load(base + 8*i + 4)
    fx.op(I::LocalGet(c_base));
    fx.op(I::LocalGet(c_i));
    fx.op(I::I32Const(8));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Const(4));
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::LocalSet(c_val));
    fx.op(I::LocalGet(c_acc));
    fx.op(I::LocalGet(c_key));
    fx.op(I::Call(seq_strcat2));
    fx.op(I::LocalSet(c_acc));
    fx.op(I::LocalGet(c_acc));
    fx.op(I::I32Const(str_colon as i32));
    fx.op(I::Call(seq_strcat2));
    fx.op(I::LocalSet(c_acc));
    fx.op(I::LocalGet(c_acc));
    fx.op(I::LocalGet(c_val));
    fx.op(I::Call(seq_to_str));
    fx.op(I::Call(seq_strcat2));
    fx.op(I::LocalSet(c_acc));
    fx.op(I::LocalGet(c_i));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(c_i));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::LocalGet(c_acc));
    fx.op(I::I32Const(str_rc as i32));
    fx.op(I::Call(seq_strcat2));
    fx.op(I::Return);
    fx.op(I::End);
    // variant: name  or  name(payload)
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_VAR));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(c_key)); // case-name str box
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::LocalSet(c_val)); // payload box (0 if none)
    fx.op(I::LocalGet(c_val));
    fx.op(I::I32Const(0));
    fx.op(I::I32Ne);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(c_key));
    fx.op(I::I32Const(str_lp as i32));
    fx.op(I::Call(seq_strcat2));
    fx.op(I::LocalSet(c_acc));
    fx.op(I::LocalGet(c_acc));
    fx.op(I::LocalGet(c_val));
    fx.op(I::Call(seq_to_str));
    fx.op(I::Call(seq_strcat2));
    fx.op(I::LocalSet(c_acc));
    fx.op(I::LocalGet(c_acc));
    fx.op(I::I32Const(str_rp as i32));
    fx.op(I::Call(seq_strcat2));
    fx.op(I::Return);
    fx.op(I::End);
    // no payload: the bare case name
    fx.op(I::LocalGet(c_key));
    fx.op(I::Return);
    fx.op(I::End);
    // cell: cell(inner)
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_CELL));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(c_val));
    fx.op(I::I32Const(str_cell as i32));
    fx.op(I::LocalGet(c_val));
    fx.op(I::Call(seq_to_str));
    fx.op(I::Call(seq_strcat2));
    fx.op(I::LocalSet(c_acc));
    fx.op(I::LocalGet(c_acc));
    fx.op(I::I32Const(str_rp as i32));
    fx.op(I::Call(seq_strcat2));
    fx.op(I::Return);
    fx.op(I::End);
    // float: format_dec's indicative six-significant-digit text
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_DEC));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    to_str_dec(
        &mut fx,
        em.h.alloc,
        em.h.box_str,
        nan_s,
        inf_s,
        ninf_s,
        d_xf,
        d_e,
        d_k,
        d_last,
        d_t,
        buf,
        i,
    );
    fx.op(I::End);
    // char: single-quoted with the common `{c:?}` escapes
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_CHAR));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    to_str_char(&mut fx, em.h.alloc, em.h.box_str, d_k, d_t, buf, i);
    fx.op(I::End);
    // anything but int from here traps
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_INT));
    fx.op(I::I32Ne);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::Unreachable);
    fx.op(I::End);
    fx.op(I::LocalGet(0));
    fx.op(I::I64Load(ma(8, 3)));
    fx.op(I::LocalSet(n));
    fx.op(I::I32Const(32));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(buf));
    fx.op(I::I32Const(32));
    fx.op(I::LocalSet(i));
    fx.op(I::LocalGet(n));
    fx.op(I::I64Const(0));
    fx.op(I::I64LtS);
    fx.op(I::LocalSet(neg));
    fx.op(I::LocalGet(neg));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I64Const(0));
    fx.op(I::LocalGet(n));
    fx.op(I::I64Sub);
    fx.op(I::LocalSet(n));
    fx.op(I::End);
    // digits, least significant first (unsigned ops so |i64::MIN| works)
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(1));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(i));
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(n));
    fx.op(I::I64Const(10));
    fx.op(I::I64RemU);
    fx.op(I::I32WrapI64);
    fx.op(I::I32Const(b'0' as i32));
    fx.op(I::I32Add);
    fx.op(I::I32Store8(ma(0, 0)));
    fx.op(I::LocalGet(n));
    fx.op(I::I64Const(10));
    fx.op(I::I64DivU);
    fx.op(I::LocalSet(n));
    fx.op(I::LocalGet(n));
    fx.op(I::I64Const(0));
    fx.op(I::I64Ne);
    fx.op(I::BrIf(0));
    fx.op(I::End);
    fx.op(I::LocalGet(neg));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(1));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(i));
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Add);
    fx.op(I::I32Const(b'-' as i32));
    fx.op(I::I32Store8(ma(0, 0)));
    fx.op(I::End);
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Add);
    fx.op(I::I32Const(32));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Sub);
    fx.op(I::Call(em.h.box_str));
    let t = em.ty_idx(vec![I32], vec![I32]);
    em.bodies.push((t, fx.finish()));
}
