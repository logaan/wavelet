use super::*;

impl Checker<'_> {
    pub(super) fn check_builtin_call(
        &self,
        name: &str,
        args: &[NodeId],
        scope: &mut Scope,
    ) -> Result<Type, String> {
        if matches!(name, "map" | "filter" | "fold") {
            return self.check_higher_order_builtin(name, args, scope);
        }
        let arg_tys = args
            .iter()
            .map(|&argument| self.check(argument, None, scope))
            .collect::<Result<Vec<_>, _>>()?;
        if name == "get"
            && arg_tys.len() == 2
            && let Type::Tuple(elements) = self.resolve_alias(&arg_tys[0])
        {
            if let Node::Int(index) = self.arena.node(args[1]) {
                return elements
                    .get(*index as usize)
                    .cloned()
                    .ok_or_else(|| format!("eval error: tuple index {index} is out of range"));
            }
            let mut elements = elements.into_iter();
            let first = elements.next().unwrap_or(Type::Unknown);
            return elements.try_fold(first, |current, element| {
                unify(&self.types, &current, &element).ok_or_else(|| {
                    "eval error: a heterogeneous tuple requires a constant index".into()
                })
            });
        }
        match name {
            "eq" | "lt" | "le" | "gt" | "ge" | "not" | "add" | "sub" | "mul" | "div" | "rem"
            | "neg" | "min" | "max" | "abs" => self.check_numeric_builtin(name, &arg_tys),
            "len" | "empty" | "get" | "put" | "push" | "concat" | "head" | "tail" | "reverse"
            | "range" | "zip" => self.check_collection_builtin(name, &arg_tys),
            "str-cat" | "upper" | "lower" | "split" | "join" | "contains" | "to-string" => {
                check_text_builtin(name, &arg_tys)
            }
            "to-u8" | "to-u16" | "to-u32" | "to-u64" | "to-s8" | "to-s16" | "to-s32" | "to-s64"
            | "to-f32" | "to-f64" | "to-char" => self.check_conversion_builtin(name, args),
            "gensym" | "expand" | "form-kind" | "rec-key" | "rec-val" => {
                Ok(form_builtin_type(name, &arg_tys))
            }
            "cell-new" | "cell-get" | "cell-set" | "drop" => {
                self.check_state_builtin(name, &arg_tys)
            }
            _ => check_constructor_builtin(name, arg_tys),
        }
    }

    pub(super) fn builtin_callback_type(
        &self,
        name: &str,
        parameters: &[Type],
    ) -> Result<Type, String> {
        match name {
            "eq" | "lt" | "le" | "gt" | "ge" | "not" | "add" | "sub" | "mul" | "div" | "rem"
            | "neg" | "min" | "max" | "abs" => self.check_numeric_builtin(name, parameters),
            "len" | "empty" | "get" | "put" | "push" | "concat" | "head" | "tail" | "reverse"
            | "range" | "zip" => self.check_collection_builtin(name, parameters),
            "str-cat" | "upper" | "lower" | "split" | "join" | "contains" | "to-string" => {
                check_text_builtin(name, parameters)
            }
            "to-char" => Ok(Type::Char),
            name if name.starts_with("to-") => Ok(Type::from_name(&name[3..])),
            _ => check_constructor_builtin(name, parameters.to_vec()),
        }
    }

    fn check_higher_order_builtin(
        &self,
        name: &str,
        args: &[NodeId],
        scope: &mut Scope,
    ) -> Result<Type, String> {
        let expected_arity = if name == "fold" { 3 } else { 2 };
        if args.len() != expected_arity {
            return Err(format!(
                "eval error: `{name}` expects {expected_arity} arguments"
            ));
        }
        let sequence = self.check(*args.last().unwrap(), None, scope)?;
        let element = match self.resolve_alias(&sequence) {
            Type::List(element) => *element,
            Type::Unknown => Type::Unknown,
            Type::Named(_) if !self.imports_resolved => Type::Unknown,
            _ => return Err(format!("eval error: `{name}` requires a list")),
        };
        let accumulator = if name == "fold" {
            Some(self.check(args[1], None, scope)?)
        } else {
            None
        };
        let parameters = match &accumulator {
            Some(initial) => vec![initial.clone(), element.clone()],
            None => vec![element.clone()],
        };
        let result = match name {
            "filter" => Type::Bool,
            "fold" => accumulator.clone().unwrap(),
            _ => Type::Unknown,
        };
        if strict()
            && !matches!(self.arena.node(args[0]), Node::Sym(_))
            && as_fn(self.arena, args[0]).is_none()
        {
            return Err(
                "eval error: callbacks must resolve to a static function or Fn literal".into(),
            );
        }
        let callback = self.check(
            args[0],
            Some(&Type::Fn(parameters, Box::new(result))),
            scope,
        )?;
        let Type::Fn(_, result) = callback else {
            return Ok(if name == "filter" {
                sequence
            } else {
                Type::Unknown
            });
        };
        Ok(match name {
            "map" => Type::List(result),
            "filter" => sequence,
            _ => *result,
        })
    }

    fn check_numeric_builtin(&self, name: &str, arg_tys: &[Type]) -> Result<Type, String> {
        match name {
            "add" | "sub" | "mul" | "div" | "rem" | "neg" | "abs" => {
                let mut result = Type::Unknown;
                let mut any_unknown = false;
                let mut seeded = false;
                for t in arg_tys {
                    if !t.numeric() {
                        return Err(format!("eval error: `{name}` requires numeric operands"));
                    }
                    if matches!(t, Type::Unknown) {
                        any_unknown = true;
                    }
                    if !seeded {
                        result = t.clone();
                        seeded = true;
                    } else if let Some(u) = unify(&self.types, &result, t) {
                        result = u;
                    } else if result.numeric() && t.numeric() {
                        result = if result.is_float() || t.is_float() {
                            Type::F64
                        } else if compatible_widening(&self.types, &result, t) {
                            result
                        } else if compatible_widening(&self.types, t, &result) {
                            t.clone()
                        } else {
                            Type::Unknown
                        };
                    } else {
                        result = Type::Unknown;
                    }
                }
                if any_unknown {
                    Ok(Type::Unknown)
                } else {
                    Ok(result)
                }
            }

            "min" | "max" => {
                let mut result = Type::Unknown;
                for t in arg_tys {
                    if !comparable(t) {
                        return Err(format!("eval error: `{name}` requires comparable operands"));
                    }
                    result = unify(&self.types, &result, t).unwrap_or(Type::Unknown);
                }
                Ok(result)
            }

            "eq" => Ok(Type::Bool),

            "lt" | "le" | "gt" | "ge" => {
                for t in arg_tys {
                    if !comparable(t) {
                        return Err(format!("eval error: `{name}` requires comparable operands"));
                    }
                }
                Ok(Type::Bool)
            }
            "not" => {
                for t in arg_tys {
                    if !matches!(t, Type::Bool | Type::Unknown) {
                        return Err("eval error: `not` requires a bool operand".to_string());
                    }
                }
                Ok(Type::Bool)
            }

            _ => Ok(Type::Unknown),
        }
    }

    fn check_collection_builtin(&self, name: &str, arg_tys: &[Type]) -> Result<Type, String> {
        match name {
            "empty" => Ok(Type::Bool),
            "len" => Ok(Type::IntLit(None)),
            "get" | "head" => Ok(elem_type(arg_tys.first())),
            "tail" | "reverse" => Ok(arg_tys.first().cloned().unwrap_or(Type::Unknown)),
            "put" if arg_tys.len() == 3 => {
                let elem = unify(&self.types, &elem_type(arg_tys.first()), &arg_tys[2])
                    .unwrap_or(Type::Unknown);
                Ok(Type::List(Box::new(elem)))
            }
            "push" if arg_tys.len() == 2 => {
                let elem = unify(&self.types, &elem_type(arg_tys.first()), &arg_tys[1])
                    .unwrap_or(Type::Unknown);
                Ok(Type::List(Box::new(elem)))
            }
            "concat" if arg_tys.len() == 2 => Ok(unify(&self.types, &arg_tys[0], &arg_tys[1])
                .unwrap_or(Type::List(Box::new(Type::Unknown)))),
            "range" => Ok(Type::List(Box::new(Type::S64))),
            "zip" if arg_tys.len() == 2 => Ok(Type::List(Box::new(Type::Tuple(vec![
                elem_type(arg_tys.first()),
                elem_type(arg_tys.get(1)),
            ])))),

            _ => Ok(Type::Unknown),
        }
    }

    fn check_conversion_builtin(&self, name: &str, args: &[NodeId]) -> Result<Type, String> {
        match name {
            "to-u8" | "to-u16" | "to-u32" | "to-u64" | "to-s8" | "to-s16" | "to-s32" | "to-s64" => {
                let ty = Type::from_name(&name[3..]);
                if let [arg] = args
                    && let Node::Int(n) = self.arena.node(*arg)
                    && !int_in_range(*n, &ty)
                {
                    return Err(format!("eval error: `{name}`: {n} out of range"));
                }
                Ok(ty)
            }
            "to-f32" => Ok(Type::F32),
            "to-f64" => Ok(Type::F64),
            "to-char" => {
                if let [arg] = args
                    && let Node::Int(n) = self.arena.node(*arg)
                    && u32::try_from(*n).ok().and_then(char::from_u32).is_none()
                {
                    return Err(format!(
                        "eval error: `to-char`: {n} is not a Unicode scalar value"
                    ));
                }
                Ok(Type::Char)
            }

            _ => Ok(Type::Unknown),
        }
    }

    fn check_state_builtin(&self, name: &str, arg_tys: &[Type]) -> Result<Type, String> {
        match name {
            "cell-new" => match arg_tys {
                [value] if strict() && contains_fn(value) => {
                    Err("eval error: cells cannot store function values".into())
                }
                [value] => Ok(Type::Cell(Box::new(mutable_type(value)))),
                _ => Err("eval error: `cell-new` expects one argument".into()),
            },
            "cell-get" => match arg_tys {
                [Type::Cell(value)] => Ok((**value).clone()),
                [Type::Unknown | Type::Named(_)] => Ok(Type::Unknown),
                _ => Err("eval error: `cell-get` expects a cell".into()),
            },
            "cell-set" => match arg_tys {
                [Type::Cell(stored), value] if compatible(&self.types, stored, value) => {
                    Ok(Type::Unit)
                }
                [Type::Unknown | Type::Named(_), _] => Ok(Type::Unit),
                [Type::Cell(_), _] => {
                    Err("eval error: `cell-set` value does not match the cell's stored type".into())
                }
                _ => Err("eval error: `cell-set` expects a cell and a value".into()),
            },
            "drop" => Ok(Type::Unit),

            _ => Ok(Type::Unknown),
        }
    }
}

fn check_text_builtin(name: &str, arg_tys: &[Type]) -> Result<Type, String> {
    match name {
        "str-cat" => {
            let operands = match arg_tys {
                [Type::List(element)] => std::slice::from_ref(element.as_ref()),
                [Type::Tuple(elements)] => elements,
                _ => arg_tys,
            };
            if operands
                .iter()
                .any(|ty| !matches!(ty, Type::String | Type::Char | Type::Unknown))
            {
                return Err("eval error: `str-cat` requires strings or characters".into());
            }
            Ok(Type::String)
        }
        "upper" | "lower" => {
            for t in arg_tys {
                if !matches!(t, Type::String | Type::Unknown) {
                    return Err(format!("eval error: `{name}` requires a string operand"));
                }
            }
            Ok(Type::String)
        }

        "split" => Ok(Type::List(Box::new(Type::String))),
        "join" | "to-string" => Ok(Type::String),
        "contains" => Ok(Type::Bool),
        _ => Ok(Type::Unknown),
    }
}

fn form_builtin_type(name: &str, arg_tys: &[Type]) -> Type {
    match name {
        "form-kind" => Type::String,
        "gensym" | "expand" | "rec-key" => Type::Tree,
        "rec-val" => match arg_tys.first() {
            Some(Type::Record(fields)) if !fields.is_empty() => fields[0].1.clone(),
            Some(Type::Tree) => Type::Tree,
            _ => Type::Unknown,
        },
        _ => Type::Unknown,
    }
}

fn check_constructor_builtin(name: &str, arg_tys: Vec<Type>) -> Result<Type, String> {
    match name {
        "some" => Ok(Type::Option(Box::new(ctor_payload(&arg_tys)))),

        "ok" => Ok(Type::Result(
            Box::new(result_ctor_payload(&arg_tys)),
            Box::new(Type::Unknown),
        )),
        "err" => Ok(Type::Result(
            Box::new(Type::Unknown),
            Box::new(result_ctor_payload(&arg_tys)),
        )),

        _ if crate::builtins::tuple_ctor_arity(name).is_some() => {
            let n = crate::builtins::tuple_ctor_arity(name).unwrap();
            if arg_tys.len() != n {
                return Err(format!(
                    "eval error: `{name}` expects {n} argument(s), got {}",
                    arg_tys.len()
                ));
            }
            if strict() && arg_tys.iter().any(contains_fn) {
                return Err("eval error: runtime tuples cannot store function values; use statically resolved functions or macros".into());
            }
            Ok(Type::Tuple(arg_tys))
        }
        _ => Ok(Type::Unknown),
    }
}
