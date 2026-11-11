"""Python 任务局部请求作用域，原生调用期间传递同一 Core 控制器。"""

from contextvars import ContextVar


_active = ContextVar("triviumdb_request_contexts", default=())


def enter(context):
    stack = _active.get()
    if len(stack) >= 32:
        raise ValueError("请求上下文嵌套超过 32 层 (Request context nesting exceeds 32 levels)")
    _active.set(stack + (context,))


def exit(context):
    stack = _active.get()
    if not stack or stack[-1] is not context:
        raise ValueError("请求上下文退出顺序无效 (Invalid request context exit order)")
    _active.set(stack[:-1])


def invoke(native, owner, operation, callback, args, kwargs):
    stack = _active.get()
    if owner == "Transaction" and operation == "__exit__":
        exception = args[1] if len(args) > 1 else kwargs.get("exc_type")
        if exception is None:
            operation = "commit"
    if not stack or not native._request_operation(owner, operation):
        return callback(*args, **kwargs)

    def call(index):
        if index == len(stack):
            return callback(*args, **kwargs)
        return native._invoke_request(stack[index], owner, operation, lambda: call(index + 1), (), {})

    return call(0)
