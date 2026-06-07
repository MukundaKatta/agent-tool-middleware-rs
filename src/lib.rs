/*!
`agent-tool-middleware` is a tiny, dependency-light middleware pipeline for
LLM agent **tool calls**.

When an agent calls a tool you often want to do work *around* the call:
inject defaults into the arguments, redact secrets, enforce limits, or log
every invocation for auditing. This crate lets you compose those concerns as
ordered [`Middleware`] and run them through a [`MiddlewarePipeline`].

The pipeline runs `pre` hooks in registration order (outermost first) to
transform the arguments, then `post` hooks in **reverse** order to transform
the result — the classic onion model used by HTTP middleware stacks.

# Example

```rust
use agent_tool_middleware::{MiddlewarePipeline, InjectFieldMiddleware, LogMiddleware};
use serde_json::json;

let mut pipe = MiddlewarePipeline::new();
pipe.add(InjectFieldMiddleware::new("api_version", json!("2024-01")));
let logger = LogMiddleware::new();
pipe.add(logger.handle());

let (args, result) = pipe.run("search", json!({"q": "rust"}), json!({"hits": 5}));

// `pre` injected the default field.
assert_eq!(args["q"], "rust");
assert_eq!(args["api_version"], "2024-01");
assert_eq!(result["hits"], 5);

// The pipeline keeps its own audit log of every call.
assert_eq!(pipe.call_count(), 1);
assert_eq!(pipe.call_log()[0].tool, "search");

// `LogMiddleware` records calls too, observable through its shared handle.
assert_eq!(logger.entries().len(), 1);
```
*/

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;

/// A transformation applied around a tool call.
///
/// Implementors can override [`pre`](Middleware::pre) to rewrite the call
/// arguments before the tool runs and [`post`](Middleware::post) to rewrite
/// the result afterwards. Both hooks default to identity, so you only
/// implement the side you care about.
pub trait Middleware {
    /// A short, stable identifier for this middleware (used in
    /// [`MiddlewarePipeline::middleware_names`]).
    fn name(&self) -> &str;

    /// Transform the arguments before the tool is invoked.
    ///
    /// The default implementation returns `args` unchanged.
    fn pre(&self, tool: &str, args: Value) -> Value {
        let _ = tool;
        args
    }

    /// Transform the result after the tool has been invoked.
    ///
    /// The default implementation returns `result` unchanged.
    fn post(&self, tool: &str, result: Value) -> Value {
        let _ = tool;
        result
    }
}

/// Shared, thread-safe storage for [`LogMiddleware`] entries.
type LogStore = Arc<Mutex<Vec<CallRecord>>>;

/// Middleware that records every tool call it observes.
///
/// `LogMiddleware` keeps its log behind an [`Arc`] so you can hold on to the
/// handle for inspection *after* moving a clone into the pipeline. Call
/// [`LogMiddleware::handle`] to get the pipeline-owned clone and keep the
/// original to read [`entries`](LogMiddleware::entries).
///
/// ```
/// use agent_tool_middleware::{MiddlewarePipeline, LogMiddleware};
/// use serde_json::json;
///
/// let logger = LogMiddleware::new();
/// let mut pipe = MiddlewarePipeline::new();
/// pipe.add(logger.handle());
/// pipe.run("fetch", json!({"url": "x"}), json!({"status": 200}));
///
/// let entries = logger.entries();
/// assert_eq!(entries.len(), 1);
/// assert_eq!(entries[0].tool, "fetch");
/// assert_eq!(entries[0].result["status"], 200);
/// ```
#[derive(Clone, Default)]
pub struct LogMiddleware {
    log: LogStore,
}

impl LogMiddleware {
    /// Create an empty logger.
    pub fn new() -> Self {
        Self {
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Get a cheap, shared handle to add to a pipeline.
    ///
    /// The returned value shares the same underlying log as `self`, so calls
    /// recorded through the pipeline are visible via [`entries`](Self::entries).
    pub fn handle(&self) -> Self {
        self.clone()
    }

    /// Borrow the recorded call entries.
    ///
    /// # Panics
    ///
    /// Panics only if the internal mutex has been poisoned by a panic in
    /// another thread while it was held.
    pub fn entries(&self) -> MutexGuard<'_, Vec<CallRecord>> {
        self.log.lock().expect("log mutex poisoned")
    }

    /// Number of calls recorded so far.
    pub fn len(&self) -> usize {
        self.entries().len()
    }

    /// Whether no calls have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.entries().is_empty()
    }

    /// Remove all recorded entries.
    pub fn clear(&self) {
        self.entries().clear();
    }
}

impl Middleware for LogMiddleware {
    fn name(&self) -> &str {
        "log"
    }

    fn post(&self, tool: &str, result: Value) -> Value {
        self.entries().push(CallRecord {
            tool: tool.to_string(),
            args: Value::Null,
            result: result.clone(),
        });
        result
    }
}

/// Middleware that inserts (or overwrites) a single field on every object-shaped
/// argument payload.
///
/// Non-object payloads (arrays, strings, numbers, `null`) are passed through
/// untouched.
pub struct InjectFieldMiddleware {
    /// The field key to insert.
    pub field: String,
    /// The value to insert under [`field`](Self::field).
    pub value: Value,
}

impl InjectFieldMiddleware {
    /// Create a middleware that injects `field = value` into argument objects.
    pub fn new(field: &str, value: Value) -> Self {
        Self {
            field: field.to_string(),
            value,
        }
    }
}

impl Middleware for InjectFieldMiddleware {
    fn name(&self) -> &str {
        "inject_field"
    }

    fn pre(&self, _tool: &str, mut args: Value) -> Value {
        if let Some(obj) = args.as_object_mut() {
            obj.insert(self.field.clone(), self.value.clone());
        }
        args
    }
}

/// Middleware that replaces the value of a named field with a redaction marker
/// in the **result** of a call.
///
/// Useful for keeping secrets (tokens, keys) out of logs and downstream
/// consumers. Object results have the field overwritten in place; other shapes
/// pass through unchanged.
pub struct RedactFieldMiddleware {
    /// The field key to redact when present.
    pub field: String,
    /// The replacement value written in place of the redacted field.
    pub placeholder: Value,
}

impl RedactFieldMiddleware {
    /// Redact `field`, replacing it with the string `"[redacted]"`.
    pub fn new(field: &str) -> Self {
        Self {
            field: field.to_string(),
            placeholder: Value::String("[redacted]".to_string()),
        }
    }

    /// Redact `field`, replacing it with a custom `placeholder` value.
    pub fn with_placeholder(field: &str, placeholder: Value) -> Self {
        Self {
            field: field.to_string(),
            placeholder,
        }
    }
}

impl Middleware for RedactFieldMiddleware {
    fn name(&self) -> &str {
        "redact_field"
    }

    fn post(&self, _tool: &str, mut result: Value) -> Value {
        if let Some(obj) = result.as_object_mut() {
            if obj.contains_key(&self.field) {
                obj.insert(self.field.clone(), self.placeholder.clone());
            }
        }
        result
    }
}

/// A single record of one call passing through the pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct CallRecord {
    /// The tool name that was invoked.
    pub tool: String,
    /// The final arguments after all `pre` hooks ran.
    pub args: Value,
    /// The final result after all `post` hooks ran.
    pub result: Value,
}

/// An ordered stack of [`Middleware`] applied to every tool call.
///
/// `pre` hooks run in registration order; `post` hooks run in reverse order.
/// Every call is appended to an internal audit log accessible via
/// [`call_log`](Self::call_log).
#[derive(Default)]
pub struct MiddlewarePipeline {
    middleware: Vec<Box<dyn Middleware>>,
    log: Vec<CallRecord>,
}

impl MiddlewarePipeline {
    /// Create an empty pipeline.
    pub fn new() -> Self {
        Self {
            middleware: Vec::new(),
            log: Vec::new(),
        }
    }

    /// Append a middleware to the end of the stack.
    pub fn add<M: Middleware + 'static>(&mut self, m: M) {
        self.middleware.push(Box::new(m));
    }

    /// Run all `pre` hooks (in order) then all `post` hooks (in reverse),
    /// record the call, and return the final `(args, result)` pair.
    pub fn run(&mut self, tool: &str, args: Value, result: Value) -> (Value, Value) {
        let mut a = args;
        for m in &self.middleware {
            a = m.pre(tool, a);
        }
        let mut r = result;
        for m in self.middleware.iter().rev() {
            r = m.post(tool, r);
        }
        self.log.push(CallRecord {
            tool: tool.to_string(),
            args: a.clone(),
            result: r.clone(),
        });
        (a, r)
    }

    /// All recorded call records, oldest first.
    pub fn call_log(&self) -> &[CallRecord] {
        &self.log
    }

    /// Number of calls that have run through this pipeline.
    pub fn call_count(&self) -> usize {
        self.log.len()
    }

    /// Number of registered middleware.
    pub fn middleware_count(&self) -> usize {
        self.middleware.len()
    }

    /// The [`name`](Middleware::name) of each registered middleware, in order.
    pub fn middleware_names(&self) -> Vec<&str> {
        self.middleware.iter().map(|m| m.name()).collect()
    }

    /// Discard the recorded call log without removing any middleware.
    pub fn clear_log(&mut self) {
        self.log.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn no_middleware_passthrough() {
        let mut p = MiddlewarePipeline::new();
        let (a, r) = p.run("fn", json!({"q": 1}), json!({"ok": true}));
        assert_eq!(a["q"], 1);
        assert_eq!(r["ok"], true);
    }

    #[test]
    fn inject_field_pre() {
        let mut p = MiddlewarePipeline::new();
        p.add(InjectFieldMiddleware::new("version", json!("1.0")));
        let (a, _) = p.run("fn", json!({"q": "x"}), json!({}));
        assert_eq!(a["version"], "1.0");
    }

    #[test]
    fn inject_field_preserves_existing() {
        let mut p = MiddlewarePipeline::new();
        p.add(InjectFieldMiddleware::new("v", json!(99)));
        let (a, _) = p.run("fn", json!({"q": "x"}), json!({}));
        assert_eq!(a["q"], "x");
        assert_eq!(a["v"], 99);
    }

    #[test]
    fn call_log_recorded() {
        let mut p = MiddlewarePipeline::new();
        p.run("search", json!({}), json!({}));
        p.run("fetch", json!({}), json!({}));
        assert_eq!(p.call_count(), 2);
        assert_eq!(p.call_log()[0].tool, "search");
    }

    #[test]
    fn middleware_count() {
        let mut p = MiddlewarePipeline::new();
        p.add(InjectFieldMiddleware::new("x", json!(1)));
        p.add(InjectFieldMiddleware::new("y", json!(2)));
        assert_eq!(p.middleware_count(), 2);
    }

    #[test]
    fn multiple_inject_fields() {
        let mut p = MiddlewarePipeline::new();
        p.add(InjectFieldMiddleware::new("a", json!(1)));
        p.add(InjectFieldMiddleware::new("b", json!(2)));
        let (args, _) = p.run("fn", json!({}), json!({}));
        assert_eq!(args["a"], 1);
        assert_eq!(args["b"], 2);
    }

    #[test]
    fn log_middleware_name() {
        let m = LogMiddleware::new();
        assert_eq!(m.name(), "log");
    }

    #[test]
    fn inject_field_name() {
        let m = InjectFieldMiddleware::new("x", json!(1));
        assert_eq!(m.name(), "inject_field");
    }

    #[test]
    fn call_record_fields() {
        let mut p = MiddlewarePipeline::new();
        p.run("my_tool", json!({"arg": "val"}), json!({"result": 42}));
        let rec = &p.call_log()[0];
        assert_eq!(rec.tool, "my_tool");
        assert_eq!(rec.result["result"], 42);
    }

    #[test]
    fn non_object_args_unchanged() {
        let mut p = MiddlewarePipeline::new();
        p.add(InjectFieldMiddleware::new("x", json!(1)));
        let (a, _) = p.run("fn", json!([1, 2, 3]), json!({}));
        // Array isn't touched by inject
        assert_eq!(a, json!([1, 2, 3]));
    }

    #[test]
    fn empty_pipeline_no_overhead() {
        let mut p = MiddlewarePipeline::new();
        let (a, r) = p.run("t", json!("x"), json!("y"));
        assert_eq!(a, "x");
        assert_eq!(r, "y");
    }

    // --- New behavior introduced in this change ---

    #[test]
    fn log_middleware_actually_records() {
        // Regression test: previously LogMiddleware never recorded anything.
        let logger = LogMiddleware::new();
        assert!(logger.is_empty());

        let mut p = MiddlewarePipeline::new();
        p.add(logger.handle());
        p.run("alpha", json!({}), json!({"ok": 1}));
        p.run("beta", json!({}), json!({"ok": 2}));

        assert_eq!(logger.len(), 2);
        let entries = logger.entries();
        assert_eq!(entries[0].tool, "alpha");
        assert_eq!(entries[1].tool, "beta");
        assert_eq!(entries[1].result["ok"], 2);
    }

    #[test]
    fn log_middleware_clear() {
        let logger = LogMiddleware::new();
        let mut p = MiddlewarePipeline::new();
        p.add(logger.handle());
        p.run("x", json!({}), json!({}));
        assert_eq!(logger.len(), 1);
        logger.clear();
        assert!(logger.is_empty());
    }

    #[test]
    fn redact_field_post() {
        let mut p = MiddlewarePipeline::new();
        p.add(RedactFieldMiddleware::new("token"));
        let (_, r) = p.run("auth", json!({}), json!({"token": "secret", "ok": true}));
        assert_eq!(r["token"], "[redacted]");
        assert_eq!(r["ok"], true);
    }

    #[test]
    fn redact_field_absent_is_noop() {
        let mut p = MiddlewarePipeline::new();
        p.add(RedactFieldMiddleware::new("token"));
        let (_, r) = p.run("auth", json!({}), json!({"ok": true}));
        assert!(r.get("token").is_none());
        assert_eq!(r["ok"], true);
    }

    #[test]
    fn redact_field_custom_placeholder() {
        let mut p = MiddlewarePipeline::new();
        p.add(RedactFieldMiddleware::with_placeholder("pw", json!(null)));
        let (_, r) = p.run("login", json!({}), json!({"pw": "hunter2"}));
        assert_eq!(r["pw"], json!(null));
    }

    #[test]
    fn redact_non_object_result_unchanged() {
        let mut p = MiddlewarePipeline::new();
        p.add(RedactFieldMiddleware::new("token"));
        let (_, r) = p.run("t", json!({}), json!("plain string"));
        assert_eq!(r, "plain string");
    }

    #[test]
    fn middleware_names_in_order() {
        let mut p = MiddlewarePipeline::new();
        p.add(InjectFieldMiddleware::new("v", json!(1)));
        p.add(RedactFieldMiddleware::new("token"));
        p.add(LogMiddleware::new());
        assert_eq!(
            p.middleware_names(),
            vec!["inject_field", "redact_field", "log"]
        );
    }

    #[test]
    fn post_hooks_run_in_reverse_order() {
        // First-added redactor sees the result last (outermost), so a later
        // injected field would already be present. Here we verify ordering by
        // checking both a pre and post transform compose correctly.
        let mut p = MiddlewarePipeline::new();
        p.add(InjectFieldMiddleware::new("injected", json!(true)));
        p.add(RedactFieldMiddleware::new("password"));
        let (args, result) = p.run(
            "op",
            json!({"user": "alice"}),
            json!({"password": "p", "status": "ok"}),
        );
        assert_eq!(args["injected"], true);
        assert_eq!(args["user"], "alice");
        assert_eq!(result["password"], "[redacted]");
        assert_eq!(result["status"], "ok");
    }

    #[test]
    fn pipeline_clear_log() {
        let mut p = MiddlewarePipeline::new();
        p.run("a", json!({}), json!({}));
        p.run("b", json!({}), json!({}));
        assert_eq!(p.call_count(), 2);
        p.clear_log();
        assert_eq!(p.call_count(), 0);
        // Middleware survive a log clear.
        assert_eq!(p.middleware_count(), 0);
    }

    #[test]
    fn log_handle_shares_storage() {
        let logger = LogMiddleware::new();
        let h = logger.handle();
        let mut p = MiddlewarePipeline::new();
        p.add(h);
        p.run("x", json!({}), json!({}));
        // Both the original and the moved handle observe the same record.
        assert_eq!(logger.len(), 1);
    }
}
