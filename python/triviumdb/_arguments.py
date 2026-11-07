"""公开签名的调用形状诊断，参数转换与数据库执行继续交给原生绑定。"""

from functools import wraps
from inspect import Parameter, Signature, isbuiltin, ismethoddescriptor, signature
from ._requests import invoke as _invoke_request


_SPECIAL_METHODS = {"__len__", "__contains__", "__repr__", "__enter__", "__exit__"}

# 构造器的实参与默认值由此元数据绑定，兼容 Python 3.9 的原生类型签名限制。
_CONSTRUCTOR_PARAMETERS = {
    "TriviumDB": (
        ("path", Parameter.empty), ("dim", 1536), ("dtype", "f32"), ("sync_mode", "normal"),
        ("load_text_index", False), ("auto_build_quiver", True), ("expected_nodes", None),
        ("memory_limit_mb", 0), ("payload_cache_mb", 64), ("payload_cache_entry_mb", 8),
        ("access_mode", "read_write"), ("missing_index_policy", "fallback"),
        ("max_query_rows", None), ("row_overflow", "throw"), ("runtime", None),
    ),
    "ResourceRuntime": (("memory_limit_mb", 0), ("max_threads", 0),
                        ("max_active_tasks", 0), ("maintenance_memory_limit_mb", 0)),
    "ConsoleSession": (("options_json", "{}"),),
    "RequestContext": (("request_id", Parameter.empty), ("timeout_ms", 0)),
}


def _issue(call_signature, args, kwargs):
    # 只向 Signature 传递占位值和原始字符串内容，避免错误诊断执行宿主协议。
    names = tuple(str.__str__(name) for name in kwargs)
    try:
        call_signature.bind(*((None,) * len(args)), **dict.fromkeys(names))
    except TypeError as error:
        parameters = tuple(call_signature.parameters.values())
        positional = tuple(value for value in parameters if value.kind in (
            Parameter.POSITIONAL_ONLY, Parameter.POSITIONAL_OR_KEYWORD))
        if len(args) > len(positional) and not any(
                value.kind == Parameter.VAR_POSITIONAL for value in parameters):
            return error, "too_many_positional", "arguments"
        accepts_keywords = any(value.kind == Parameter.VAR_KEYWORD for value in parameters)
        for index, parameter in enumerate(positional):
            if parameter.name in names:
                if parameter.kind == Parameter.POSITIONAL_ONLY and not accepts_keywords:
                    return error, "positional_only", parameter.name
                if parameter.kind == Parameter.POSITIONAL_OR_KEYWORD and index < len(args):
                    return error, "multiple_values", parameter.name
        known = {value.name for value in parameters if value.kind in (
            Parameter.POSITIONAL_OR_KEYWORD, Parameter.KEYWORD_ONLY)}
        unknown = sorted(set(names) - known)
        if unknown and not accepts_keywords:
            return error, "unknown_keyword", unknown[0]
        supplied = {value.name for value in positional[:len(args)]}
        supplied.update(known.intersection(names))
        for parameter in parameters:
            if (parameter.kind in (Parameter.POSITIONAL_ONLY, Parameter.POSITIONAL_OR_KEYWORD,
                                   Parameter.KEYWORD_ONLY)
                    and parameter.default is Parameter.empty and parameter.name not in supplied):
                return error, "missing", parameter.name
        return error, "invalid_arguments", "arguments"
    return None


def _wrap(original, owner, operation, annotate, native):
    call_signature = signature(original)
    parameters = tuple(call_signature.parameters.values())
    positional = tuple(value for value in parameters if value.kind in (
        Parameter.POSITIONAL_ONLY, Parameter.POSITIONAL_OR_KEYWORD))
    minimum = sum(value.default is Parameter.empty for value in positional)
    maximum = None if any(value.kind == Parameter.VAR_POSITIONAL for value in parameters) else len(positional)
    required_keywords = any(
        value.kind == Parameter.KEYWORD_ONLY and value.default is Parameter.empty
        for value in parameters)

    @wraps(original)
    def call(*args, **kwargs):
        # 普通位置调用复用原生快路径，其他形状在值转换与方法执行之前验证。
        if (kwargs or len(args) < minimum
                or (maximum is not None and len(args) > maximum) or required_keywords):
            issue = _issue(call_signature, args, kwargs)
            if issue is not None:
                error, kind, field = issue
                raise annotate(error, owner, operation, field, kind)
        return _invoke_request(native, owner, operation, original, args, kwargs)

    call.__signature__ = call_signature
    call.__text_signature__ = original.__text_signature__
    return call


def install(native):
    annotate = native._call_argument_error
    constructors = {name: Signature(Parameter(field, Parameter.POSITIONAL_OR_KEYWORD, default=default)
                                    for field, default in parameters)
                    for name, parameters in _CONSTRUCTOR_PARAMETERS.items()}
    for name, call_signature in constructors.items():
        getattr(native, name).__signature__ = call_signature

    def bind_constructor(owner, args, kwargs):
        # 原生构造器收集实参后复用公开签名；值转换仍由原生绑定执行。
        keywords = {str.__str__(name): value for name, value in (kwargs or {}).items()}
        call_signature = constructors[owner]
        issue = _issue(call_signature, args, keywords)
        if issue is not None:
            error, kind, field = issue
            raise annotate(error, owner, "__new__", field, kind)
        arguments = call_signature.bind(*args, **keywords)
        arguments.apply_defaults()
        return arguments.arguments

    native._bind_constructor = bind_constructor
    for name in native.__all__:
        member = getattr(native, name)
        if isinstance(member, type) and not issubclass(member, BaseException):
            for operation, original in tuple(member.__dict__.items()):
                if operation.startswith("__") and operation not in _SPECIAL_METHODS:
                    continue
                if ismethoddescriptor(original) and original.__text_signature__ is not None:
                    setattr(member, operation, _wrap(original, name, operation, annotate, native))
        elif isbuiltin(member):
            setattr(native, name, _wrap(member, "module", name, annotate, native))
