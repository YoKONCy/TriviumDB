//! JavaScript 持有 Hook 强引用；原生侧使用弱引用并在所属事件线程清理。

use napi::bindgen_prelude::External;
use napi::threadsafe_function::{
    ThreadSafeCallContext, ThreadsafeFunction, ThreadsafeFunctionCallMode,
};
use napi::{Env, JsObject, JsUnknown, NapiRaw, NapiValue};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

const OWNER_PROPERTY: &str = "__tdbNativeHook";
const CLEANUP_PROPERTY: &str = "__tdbHookCleanup";

struct WeakRefs {
    owner: usize,
    holder: usize,
}

fn weak(env: Env, object: &JsObject) -> napi::Result<usize> {
    let mut reference = std::ptr::null_mut();
    // SAFETY: env 与 object 来自当前 JavaScript 调用；引用计数为零，生命周期由 Cleanup 独占管理。
    let status =
        unsafe { napi::sys::napi_create_reference(env.raw(), object.raw(), 0, &mut reference) };
    if status != napi::sys::Status::napi_ok {
        return Err(napi::Error::new(status.into(), "创建 Hook 弱引用失败"));
    }
    Ok(reference as usize)
}

fn object(env: Env, reference: usize) -> napi::Result<Option<JsObject>> {
    let mut value = std::ptr::null_mut();
    // SAFETY: 仅在引用所属的事件线程调用；Cleanup 取出引用前不允许删除，同步调用期间 env 有效。
    let status = unsafe {
        napi::sys::napi_get_reference_value(env.raw(), reference as napi::sys::napi_ref, &mut value)
    };
    if status != napi::sys::Status::napi_ok {
        return Err(napi::Error::new(status.into(), "读取 Hook 弱引用失败"));
    }
    if value.is_null() {
        return Ok(None);
    }
    // SAFETY: 该引用仅由 weak() 从 JsObject 建立，非空句柄仍受当前 N-API 调用作用域保护。
    unsafe { JsObject::from_raw(env.raw(), value).map(Some) }
}

fn delete(env: Env, reference: usize) {
    // SAFETY: 引用由 Cleanup 的 Mutex<Option> 保证只取出一次；仅在所属事件线程和有效 env 中删除。
    let status =
        unsafe { napi::sys::napi_delete_reference(env.raw(), reference as napi::sys::napi_ref) };
    if status != napi::sys::Status::napi_ok {
        tracing::debug!(status, "Hook 弱引用随环境退出清理");
    }
}

impl WeakRefs {
    fn release(self, env: Env) {
        let removed = (|| -> napi::Result<()> {
            let Some(mut holder) = object(env, self.holder)? else {
                return Ok(());
            };
            // Object.freeze(owner) 会固定外层属性；清空独立持有者仍可解除 Hook 强引用。
            holder.delete_named_property("hook")?;
            if let Some(mut owner) = object(env, self.owner)?
                && owner.has_own_property(OWNER_PROPERTY)?
            {
                let current: JsObject = owner.get_named_property(OWNER_PROPERTY)?;
                // 替换后的旧 Hook 不能移除新 Hook 的持有者。
                if env.strict_equals(current, holder)? {
                    owner.delete_named_property(OWNER_PROPERTY)?;
                }
            }
            Ok(())
        })();
        if let Err(error) = removed {
            tracing::debug!(%error, "清理 Hook 持有者属性失败");
        }
        delete(env, self.holder);
        delete(env, self.owner);
    }
}

pub(super) struct Cleanup {
    requested: AtomicBool,
    refs: Mutex<Option<WeakRefs>>,
}

impl Cleanup {
    fn flush(&self, env: Env) {
        if self.requested.load(Ordering::Acquire) {
            let refs = self.refs.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(refs) = refs {
                refs.release(env);
            }
        }
    }
}

pub(super) struct HookReference {
    env: Env,
    owner: std::thread::ThreadId,
    cleanup: Arc<Cleanup>,
    wake: ThreadsafeFunction<()>,
}

// SAFETY: 原生线程只设置清理标志或投递线程安全通知；N-API 访问均检查 owner 或由所属事件线程执行。
unsafe impl Send for HookReference {}
// SAFETY: 弱引用的取出、读取和销毁由 Mutex 串行化，后台线程不会调用 N-API 对象接口。
unsafe impl Sync for HookReference {}

impl HookReference {
    pub(super) fn new(env: Env, mut owner: JsObject, hook: JsObject) -> napi::Result<Self> {
        let cleanup = Arc::new(Cleanup {
            requested: AtomicBool::new(false),
            refs: Mutex::new(None),
        });
        let pending = cleanup.clone();
        let noop = env.create_function_from_closure("tdbHookCleanup", |_| Ok(()))?;
        let mut wake =
            env.create_threadsafe_function(&noop, 0, move |ctx: ThreadSafeCallContext<()>| {
                pending.flush(ctx.env);
                Ok(Vec::<JsUnknown>::new())
            })?;
        wake.unref(&env)?;
        let mut holder = env.create_object()?;
        holder.set_named_property("hook", hook)?;
        holder.set_named_property(CLEANUP_PROPERTY, External::new(cleanup.clone()))?;
        let property = napi::Property::new(OWNER_PROPERTY)?;
        let owner_ref = weak(env, &owner)?;
        let holder_ref = match weak(env, &holder) {
            Ok(reference) => reference,
            Err(error) => {
                delete(env, owner_ref);
                return Err(error);
            }
        };
        *cleanup.refs.lock().unwrap_or_else(|e| e.into_inner()) = Some(WeakRefs {
            owner: owner_ref,
            holder: holder_ref,
        });
        if let Err(error) = owner.define_properties(&[property
            .with_value(&holder)
            .with_property_attributes(napi::PropertyAttributes::Configurable)])
        {
            cleanup.requested.store(true, Ordering::Release);
            cleanup.flush(env);
            return Err(error);
        }
        Ok(Self {
            env,
            owner: std::thread::current().id(),
            cleanup,
            wake,
        })
    }

    pub(super) fn get(&self) -> napi::Result<JsObject> {
        if std::thread::current().id() != self.owner {
            return Err(napi::Error::from_reason(
                "Node Hook 只能在注册它的 JavaScript 线程执行",
            ));
        }
        let reference = self
            .cleanup
            .refs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .ok_or_else(|| napi::Error::from_reason("Node Hook 已释放"))?
            .holder;
        let holder = object(self.env, reference)?
            .ok_or_else(|| napi::Error::from_reason("Node Hook 已被回收"))?;
        holder.get_named_property("hook")
    }
}

impl Drop for HookReference {
    fn drop(&mut self) {
        self.cleanup.requested.store(true, Ordering::Release);
        if std::thread::current().id() == self.owner {
            self.cleanup.flush(self.env);
        } else {
            let _ = self
                .wake
                .call(Ok(()), ThreadsafeFunctionCallMode::NonBlocking);
        }
    }
}

/// 同步关闭等待结束时，在返回 JavaScript 前清空后台线程已登记的引用清理。
pub(super) fn flush(env: Env, owner: &JsObject) -> napi::Result<()> {
    if owner.has_own_property(OWNER_PROPERTY)? {
        let holder: JsObject = owner.get_named_property(OWNER_PROPERTY)?;
        let cleanup: External<Arc<Cleanup>> = holder.get_named_property(CLEANUP_PROPERTY)?;
        cleanup.flush(env);
    }
    Ok(())
}
