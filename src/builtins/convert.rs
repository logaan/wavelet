use super::{R, numeric::want_num};
use crate::interp::err;
use crate::value::{Value, print_value};

pub(super) fn call(name: &str, arg: Value) -> R<Value> {
    match name {
        "to-u8" | "to-u16" | "to-u32" | "to-u64" | "to-s8" | "to-s16" | "to-s32" | "to-s64" => {
            let n = match &arg {
                Value::Int(n) => *n,
                Value::Dec(f) if f.fract() == 0.0 => *f as i64,
                Value::Char(c) => *c as i64,
                other => {
                    return err(format!(
                        "`{name}` expects a number, got {}",
                        print_value(other)
                    ));
                }
            };
            let ok = match name {
                "to-u8" => (0..=u8::MAX as i64).contains(&n),
                "to-u16" => (0..=u16::MAX as i64).contains(&n),
                "to-u32" => (0..=u32::MAX as i64).contains(&n),
                "to-u64" => n >= 0,
                "to-s8" => (i8::MIN as i64..=i8::MAX as i64).contains(&n),
                "to-s16" => (i16::MIN as i64..=i16::MAX as i64).contains(&n),
                "to-s32" => (i32::MIN as i64..=i32::MAX as i64).contains(&n),
                _ => true,
            };
            if ok {
                Ok(Value::Int(n))
            } else {
                err(format!("`{name}`: {n} out of range"))
            }
        }
        "to-f32" | "to-f64" => Ok(Value::Dec(want_num(&arg, name)?.as_float())),
        "to-char" => match &arg {
            Value::Char(_) => Ok(arg.clone()),
            Value::Int(n) => match u32::try_from(*n).ok().and_then(char::from_u32) {
                Some(c) => Ok(Value::Char(c)),
                None => err(format!("`to-char`: {n} is not a Unicode scalar value")),
            },
            other => err(format!(
                "`to-char` expects an int, got {}",
                print_value(other)
            )),
        },
        _ => unreachable!("builtin routed to the wrong family"),
    }
}
