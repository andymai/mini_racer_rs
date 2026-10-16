use mini_racer_wire::marker;
use v8::ValueSerializerHelper;

// mini_racer's filter (mini_racer_v8.cc, MIT, Copyright (c) 2016-2019 the
// mini_racer project authors), byte for byte. It runs in a context user code
// never touched, so tampered prototypes cannot change how a value is copied.
const FILTER_SOURCE: &str = r#"
;(function($globalThis) {
    const {Map: $Map, Set: $Set} = $globalThis
    const sentinel = {}
    return function filter(v) {
        if (typeof v === "function")
            return sentinel
        if (typeof v !== "object" || v === null)
            return v
        if (v instanceof $Map) {
            const m = new Map()
            for (let [k, t] of Map.prototype.entries.call(v)) {
                t = filter(t)
                if (t !== sentinel)
                    m.set(k, t)
            }
            return m
        } else if (v instanceof $Set) {
            const s = new Set()
            for (let t of Set.prototype.values.call(v)) {
                t = filter(t)
                if (t !== sentinel)
                    s.add(t)
            }
            return s
        } else {
            const o = Array.isArray(v) ? [] : {}
            const pds = Object.getOwnPropertyDescriptors(v)
            for (const [k, d] of Object.entries(pds)) {
                if (!d.enumerable)
                    continue
                let t = d.value
                if (d.get) {
                    // *not* d.get.call(...), may have been tampered with
                    t = Function.prototype.call.call(d.get, v, k)
                }
                t = filter(t)
                if (t !== sentinel)
                    Object.defineProperty(o, k, {value: t, enumerable: true})
            }
            return o
        }
    }
})
"#;

pub type Tc<'a, 'b, 'c, 'i> = v8::PinnedRef<'a, v8::TryCatch<'b, 'c, v8::HandleScope<'i>>>;

/// The context `mini_racer` copies uncloneable values in, and its filter.
pub struct Filter {
    context: v8::Global<v8::Context>,
    function: v8::Global<v8::Function>,
}

pub fn origin<'s>(
    scope: &v8::PinScope<'s, '_>,
    name: v8::Local<'s, v8::Value>,
) -> v8::ScriptOrigin<'s> {
    v8::ScriptOrigin::new(scope, name, 0, 0, false, 0, None, false, false, false, None)
}

/// Creates the safe context, then the user context, as `mini_racer` does. The
/// filter's setup reads the user context's `Map` and `Set` once, under a
/// shared security token that is revoked right after.
pub fn contexts(scope: &mut v8::PinScope<'_, '_, ()>) -> Option<(v8::Global<v8::Context>, Filter)> {
    let safe = v8::Context::new(scope, v8::ContextOptions::default());
    let user = v8::Context::new(scope, v8::ContextOptions::default());
    let function = {
        let scope = &mut v8::ContextScope::new(scope, safe);
        let source = v8::String::new(scope, FILTER_SOURCE)?;
        let name = v8::String::new(scope, "safe_context_script.js")?;
        let origin = origin(scope, name.into());
        let setup = v8::Script::compile(scope, source, Some(&origin))?.run(scope)?;
        let setup = v8::Local::<v8::Function>::try_from(setup).ok()?;
        safe.set_security_token(user.get_security_token(scope));
        let receiver = v8::undefined(scope).into();
        let global = user.global(scope).into();
        let filter = setup.call(scope, receiver, &[global]);
        safe.use_default_security_token();
        v8::Local::<v8::Function>::try_from(filter?).ok()?
    };
    Some((
        v8::Global::new(scope, user),
        Filter {
            context: v8::Global::new(scope, safe),
            function: v8::Global::new(scope, function),
        },
    ))
}

/// What `mini_racer` does to a result before serializing it: proxies give way
/// to their targets, symbols to their descriptions, functions to the marker
/// its Ruby side reads as `JavaScriptFunction`, and weak collections and
/// iterators to the entries they would show.
pub fn sanitize<'s>(
    scope: &v8::PinScope<'s, '_>,
    mut value: v8::Local<'s, v8::Value>,
) -> v8::Local<'s, v8::Value> {
    while let Ok(proxy) = v8::Local::<v8::Proxy>::try_from(value) {
        value = proxy.get_target(scope);
    }
    if let Ok(symbol) = v8::Local::<v8::Symbol>::try_from(value) {
        return symbol.description(scope);
    }
    if value.is_function() {
        return v8::String::new_from_two_byte(
            scope,
            &marker::JAVASCRIPT_FUNCTION,
            v8::NewStringType::Normal,
        )
        .map_or(value, Into::into);
    }
    if (value.is_weak_map()
        || value.is_weak_set()
        || value.is_map_iterator()
        || value.is_set_iterator())
        && let Ok(object) = v8::Local::<v8::Object>::try_from(value)
        && let (Some(entries), _) = object.preview_entries(scope)
    {
        return entries.into();
    }
    value
}

struct Delegate;

impl Delegate {
    // V8's message when it has no delegate, which mini_racer's errors carry.
    fn refuse(scope: &v8::PinScope<'_, '_>, object: v8::Local<'_, v8::Object>) {
        let message = format!(
            "#<{}> could not be cloned.",
            object.get_constructor_name().to_rust_string_lossy(scope)
        );
        if let Some(message) = v8::String::new(scope, &message) {
            let exception = v8::Exception::error(scope, message);
            scope.throw_exception(exception);
        }
    }
}

impl v8::ValueSerializerImpl for Delegate {
    fn throw_data_clone_error<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        message: v8::Local<'s, v8::String>,
    ) {
        let exception = v8::Exception::error(scope, message);
        scope.throw_exception(exception);
    }

    // Without an exception V8 would report success having written nothing.
    fn write_host_object<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        object: v8::Local<'s, v8::Object>,
        _serializer: &dyn ValueSerializerHelper,
    ) -> Option<bool> {
        Self::refuse(scope, object);
        None
    }

    fn get_wasm_module_transfer_id(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        module: v8::Local<v8::WasmModuleObject>,
    ) -> Option<u32> {
        Self::refuse(scope, module.into());
        None
    }
}

fn write(tc: &Tc<'_, '_, '_, '_>, value: v8::Local<'_, v8::Value>) -> Option<Vec<u8>> {
    let context = tc.get_current_context();
    let serializer = v8::ValueSerializer::new(tc, Box::new(Delegate));
    serializer.write_header();
    match serializer.write_value(context, value) {
        Some(true) => Some(serializer.release()),
        _ => None,
    }
}

/// Serializes `value` as `mini_racer` replies. A value V8 refuses goes through
/// the filter, which runs its getters a second time, and is serialized again.
/// A failure whose message says the value could not be cloned, from either
/// step, becomes `mini_racer`'s `{"error": <exception>}`. `None` leaves the
/// exception in `tc`.
pub fn reply(
    tc: &mut Tc<'_, '_, '_, '_>,
    filter: &Filter,
    value: v8::Local<'_, v8::Value>,
) -> Option<Vec<u8>> {
    if let Some(bytes) = write(tc, value) {
        return Some(bytes);
    }
    if !tc.can_continue() {
        return None;
    }
    tc.reset();
    let function = v8::Local::new(tc, &filter.function);
    let context = v8::Local::new(tc, &filter.context);
    let filtered = {
        let scope = &mut v8::ContextScope::new(&mut **tc, context);
        let receiver = v8::undefined(scope).into();
        function.call(scope, receiver, &[value])
    };
    if let Some(bytes) = filtered.and_then(|filtered| write(tc, filtered)) {
        return Some(bytes);
    }
    if !tc.can_continue() {
        return None;
    }
    // mini_racer tells a clone failure from any other by this text.
    let message = tc.exception()?.to_string(tc)?;
    if !message
        .to_rust_string_lossy(tc)
        .contains("could not be cloned")
    {
        return None;
    }
    tc.reset();
    let error = {
        let scope = &mut v8::ContextScope::new(&mut **tc, context);
        let object = v8::Object::new(scope);
        let key = v8::String::new(scope, "error")?;
        object.set(scope, key.into(), message.into())?;
        object
    };
    write(tc, error.into())
}
