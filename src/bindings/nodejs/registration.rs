//! 原生模块导出前统一数据库类名，供各职责实现块共同注册方法。

// napi-rs 2 的普通实现块按 Rust 名称推导 TriviumDb，构造器使用显式 TriviumDB。
// 框架先执行导出回调，再装配全部类。在此阶段固定类名，保留各实现块已登记的属性。
#[napi::bindgen_prelude::ctor]
fn register_database_export() {
    napi::bindgen_prelude::register_module_export(None, "TriviumDB\0", prepare_database_export);
}

unsafe fn prepare_database_export(env: napi::sys::napi_env) -> napi::Result<napi::sys::napi_value> {
    napi::__private::register_class("TriviumDB", None, "TriviumDB\0", Vec::new());
    let mut undefined = std::ptr::null_mut();
    // SAFETY: 框架在当前 Node 事件线程传入有效 env，输出地址属于本次调用的栈。
    // 临时值由框架持有，随后同一导出名由类装配阶段写入完整构造器。
    let status = unsafe { napi::sys::napi_get_undefined(env, &mut undefined) };
    napi::check_status!(
        status,
        "读取模块初始化值失败 (Failed to read module initialization value)"
    )?;
    Ok(undefined)
}
