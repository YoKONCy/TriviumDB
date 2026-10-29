//! Node JSON 边界统一转换：属性名使用带长度的 JS 字符串，保留 NUL 与自有属性。

use napi::bindgen_prelude::{FromNapiValue, ToNapiValue, TypeName, ValidateNapiValue};
use napi::{Env, JsObject, JsUnknown, NapiRaw, NapiValue, ValueType, sys};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct NodeJson(pub Value);

impl From<Value> for NodeJson {
    fn from(value: Value) -> Self {
        Self(value)
    }
}
impl std::ops::Deref for NodeJson {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}
impl TypeName for NodeJson {
    fn type_name() -> &'static str {
        "serde_json::Value"
    }
    fn value_type() -> ValueType {
        ValueType::Unknown
    }
}
impl ValidateNapiValue for NodeJson {}

impl FromNapiValue for NodeJson {
    unsafe fn from_napi_value(env: sys::napi_env, value: sys::napi_value) -> napi::Result<Self> {
        // SAFETY: napi 在当前 JS 线程传入有效 env/value；所有子句柄只在当前调用作用域使用。
        let (env, value) = unsafe {
            (
                Env::from_raw(env),
                JsUnknown::from_raw_unchecked(env, value),
            )
        };
        read_json(env, value, 0).map(Self)
    }
}
impl ToNapiValue for NodeJson {
    unsafe fn to_napi_value(env: sys::napi_env, value: Self) -> napi::Result<sys::napi_value> {
        // SAFETY: 转换在 napi 提供的有效 env 所在线程同步完成，返回值由 JS 运行时持有。
        unsafe { write_json(Env::from_raw(env), value.0).map(|value| value.raw()) }
    }
}

fn read_json(env: Env, value: JsUnknown, depth: usize) -> napi::Result<Value> {
    // 该上限保护绑定递归；写入契约仍由内核的 JSON 校验统一决定。
    if depth > 256 {
        return Err(napi::Error::new(
            napi::Status::InvalidArg,
            "JS JSON 转换深度超过 256 (JS JSON conversion depth exceeds 256)",
        ));
    }
    match value.get_type()? {
        ValueType::Object => {
            // SAFETY: 已确认 Object 类型；句柄和 env 均来自当前作用域。
            let object = unsafe { JsObject::from_raw_unchecked(env.raw(), value.raw()) };
            if object.is_array()? {
                let mut values = Vec::new();
                for index in 0..object.get_array_length()? {
                    values.push(read_json(env, object.get_element(index)?, depth + 1)?);
                }
                Ok(Value::Array(values))
            } else {
                let keys = object.get_property_names()?;
                let mut values = serde_json::Map::new();
                for index in 0..keys.get_array_length()? {
                    let key = keys
                        .get_element::<napi::JsString>(index)?
                        .into_utf8()?
                        .as_str()?
                        .to_owned();
                    let value = object.get_property(env.create_string(&key)?)?;
                    values.insert(key, read_json(env, value, depth + 1)?);
                }
                Ok(Value::Object(values))
            }
        }
        ValueType::BigInt => Err(napi::Error::new(
            napi::Status::InvalidArg,
            "Payload BigInt 请使用十进制字符串 (Represent Payload BigInt as a decimal string)",
        )),
        // 标量沿用 napi 既有数值与非法值边界，避免经 JSON.stringify 改写 NaN 等输入。
        _ => unsafe {
            // SAFETY: 句柄类型已由 napi 检测，标量转换不持有跨作用域引用。
            Value::from_napi_value(env.raw(), value.raw())
        },
    }
}

fn write_json(env: Env, value: Value) -> napi::Result<JsUnknown> {
    match value {
        Value::Array(values) => {
            let mut array = env.create_array_with_length(values.len())?;
            for (index, value) in values.into_iter().enumerate() {
                let index = u32::try_from(index).map_err(|_| {
                    napi::Error::from_reason(
                        "JSON 数组超出 JS 索引范围 (JSON array exceeds JS index range)",
                    )
                })?;
                array.set_element(index, write_json(env, value)?)?;
            }
            Ok(array.into_unknown())
        }
        Value::Object(values) => {
            let object = env.create_object()?;
            for (key, value) in values {
                let name = env.create_string(&key)?;
                let value = write_json(env, value)?;
                // SAFETY: name/value/object 同属当前 env；描述符指针只在同步调用期间借用。
                // 通过自有数据属性保存 __proto__ 等名称，避免触发对象原型 setter。
                let status = unsafe {
                    let property = sys::napi_property_descriptor {
                        utf8name: std::ptr::null(),
                        name: name.raw(),
                        value: value.raw(),
                        method: None,
                        getter: None,
                        setter: None,
                        attributes: sys::PropertyAttributes::writable
                            | sys::PropertyAttributes::enumerable
                            | sys::PropertyAttributes::configurable,
                        data: std::ptr::null_mut(),
                    };
                    sys::napi_define_properties(env.raw(), object.raw(), 1, &property)
                };
                if status != 0 {
                    return Err(napi::Error::new(
                        napi::Status::from(status),
                        "JSON 属性创建失败 (JSON property creation failed)",
                    ));
                }
            }
            Ok(object.into_unknown())
        }
        value => {
            // SAFETY: 标量在当前 env 内转换；返回句柄由 JS 运行时接管。
            unsafe {
                Value::to_napi_value(env.raw(), value)
                    .map(|value| JsUnknown::from_raw_unchecked(env.raw(), value))
            }
        }
    }
}
