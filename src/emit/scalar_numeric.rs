use super::*;

impl Emitter<'_> {
    pub(crate) fn scalar_arithmetic(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        args: &[NodeId],
    ) -> Result<Option<Scalar>, String> {
        use Scalar::*;
        match name {
            "add" | "sub" | "mul" | "div" | "rem" if args.len() == 2 => {
                let (Some(ka), Some(kb)) = (self.node_scalar(args[0]), self.node_scalar(args[1]))
                else {
                    return Ok(None);
                };
                if !matches!(ka, Int | Float) || !matches!(kb, Int | Float) {
                    return Ok(None);
                }
                if ka == Float || kb == Float {
                    self.expr_scalar(fx, args[0], Float)?;
                    self.expr_scalar(fx, args[1], Float)?;
                    match name {
                        "add" => fx.op(I::F64Add),
                        "sub" => fx.op(I::F64Sub),
                        "mul" => fx.op(I::F64Mul),
                        "div" => fx.op(I::F64Div),
                        _ => return Err("`rem` requires integer operands".into()),
                    }
                    Ok(Some(Float))
                } else {
                    self.expr_scalar(fx, args[0], Int)?;
                    self.expr_scalar(fx, args[1], Int)?;
                    fx.op(match name {
                        "add" => I::I64Add,
                        "sub" => I::I64Sub,
                        "mul" => I::I64Mul,
                        "div" => I::I64DivS,
                        _ => I::I64RemS,
                    });
                    Ok(Some(Int))
                }
            }
            "neg" if args.len() == 1 => match self.node_scalar(args[0]) {
                Some(Int) => {
                    fx.op(I::I64Const(0));
                    self.expr_scalar(fx, args[0], Int)?;
                    fx.op(I::I64Sub);
                    Ok(Some(Int))
                }
                Some(Float) => {
                    self.expr_scalar(fx, args[0], Float)?;
                    fx.op(I::F64Neg);
                    Ok(Some(Float))
                }
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    pub(crate) fn scalar_extrema(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        args: &[NodeId],
    ) -> Result<Option<Scalar>, String> {
        use Scalar::*;
        match name {
            "abs" if args.len() == 1 => match self.node_scalar(args[0]) {
                Some(Int) => {
                    let value = fx.local(ValType::I64);
                    self.expr_scalar(fx, args[0], Int)?;
                    fx.op(I::LocalTee(value));
                    fx.op(I::I64Const(0));
                    fx.op(I::I64LtS);
                    fx.op(I::If(BlockType::Result(ValType::I64)));
                    fx.op(I::I64Const(0));
                    fx.op(I::LocalGet(value));
                    fx.op(I::I64Sub);
                    fx.op(I::Else);
                    fx.op(I::LocalGet(value));
                    fx.op(I::End);
                    Ok(Some(Int))
                }
                Some(Float) => {
                    self.expr_scalar(fx, args[0], Float)?;
                    fx.op(I::F64Abs);
                    Ok(Some(Float))
                }
                _ => Ok(None),
            },
            "min" | "max" if args.len() == 2 => {
                let (Some(ka), Some(kb)) = (self.node_scalar(args[0]), self.node_scalar(args[1]))
                else {
                    return Ok(None);
                };
                match (ka, kb) {
                    (Int, Int) | (Char, Char) => {
                        let a = fx.local(ValType::I64);
                        let b = fx.local(ValType::I64);
                        self.expr_scalar(fx, args[0], ka)?;
                        fx.op(I::LocalSet(a));
                        self.expr_scalar(fx, args[1], kb)?;
                        fx.op(I::LocalSet(b));
                        fx.op(I::LocalGet(a));
                        fx.op(I::LocalGet(b));
                        fx.op(I::LocalGet(a));
                        fx.op(I::LocalGet(b));
                        fx.op(if name == "min" { I::I64LtS } else { I::I64GtS });
                        fx.op(I::Select);
                        Ok(Some(ka))
                    }
                    (Int | Float, Int | Float) => {
                        self.expr_scalar(fx, args[0], Float)?;
                        self.expr_scalar(fx, args[1], Float)?;
                        fx.op(if name == "min" { I::F64Min } else { I::F64Max });
                        Ok(Some(Float))
                    }
                    _ => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn scalar_order(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        args: &[NodeId],
    ) -> Result<Option<Scalar>, String> {
        use Scalar::*;
        match name {
            "lt" | "le" | "gt" | "ge" if args.len() == 2 => {
                let (Some(ka), Some(kb)) = (self.node_scalar(args[0]), self.node_scalar(args[1]))
                else {
                    return Ok(None);
                };
                match (ka, kb) {
                    (Char, Char) => {
                        self.expr_scalar(fx, args[0], Char)?;
                        self.expr_scalar(fx, args[1], Char)?;
                    }
                    (Int, Int) => {
                        self.expr_scalar(fx, args[0], Int)?;
                        self.expr_scalar(fx, args[1], Int)?;
                    }
                    (Int | Float, Int | Float) => {
                        self.expr_scalar(fx, args[0], Float)?;
                        self.expr_scalar(fx, args[1], Float)?;
                        fx.op(match name {
                            "lt" => I::F64Lt,
                            "le" => I::F64Le,
                            "gt" => I::F64Gt,
                            _ => I::F64Ge,
                        });
                        return Ok(Some(Bool));
                    }

                    _ => return Ok(None),
                }
                fx.op(match name {
                    "lt" => I::I64LtS,
                    "le" => I::I64LeS,
                    "gt" => I::I64GtS,
                    _ => I::I64GeS,
                });
                Ok(Some(Bool))
            }
            _ => Ok(None),
        }
    }
}
