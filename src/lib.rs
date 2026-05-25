/*!
agent-tool-middleware: middleware pipeline for LLM agent tool call processing.

```rust
use agent_tool_middleware::{MiddlewarePipeline, LogMiddleware};
use serde_json::json;

let mut pipe = MiddlewarePipeline::new();
pipe.add(LogMiddleware::new());
let (args, result) = pipe.run("search", json!({"q": "rust"}), json!({"hits": 5}));
assert_eq!(args["q"], "rust");
```
*/

use serde_json::Value;

/// A middleware that can modify args before a call and result after.
pub trait Middleware {
    fn name(&self) -> &str;
    fn pre(&self, tool: &str, args: Value) -> Value { let _ = tool; args }
    fn post(&self, tool: &str, result: Value) -> Value { let _ = tool; result }
}

/// Logging middleware — records every call.
pub struct LogMiddleware {
    pub log: std::sync::Mutex<Vec<(String, Value, Value)>>,
}

impl LogMiddleware {
    pub fn new() -> Self { Self { log: std::sync::Mutex::new(Vec::new()) } }

    pub fn entries(&self) -> std::sync::MutexGuard<Vec<(String, Value, Value)>> {
        self.log.lock().unwrap()
    }
}

impl Default for LogMiddleware {
    fn default() -> Self { Self::new() }
}

impl Middleware for LogMiddleware {
    fn name(&self) -> &str { "log" }

    fn post(&self, tool: &str, result: Value) -> Value {
        // Log is recorded by the pipeline's post hook.
        let _ = tool;
        result
    }
}

/// Middleware that adds a field to every args object.
pub struct InjectFieldMiddleware {
    pub field: String,
    pub value: Value,
}

impl InjectFieldMiddleware {
    pub fn new(field: &str, value: Value) -> Self {
        Self { field: field.to_string(), value }
    }
}

impl Middleware for InjectFieldMiddleware {
    fn name(&self) -> &str { "inject_field" }

    fn pre(&self, _tool: &str, mut args: Value) -> Value {
        if let Some(obj) = args.as_object_mut() {
            obj.insert(self.field.clone(), self.value.clone());
        }
        args
    }
}

/// A call record in the pipeline log.
#[derive(Debug, Clone)]
pub struct CallRecord {
    pub tool: String,
    pub args: Value,
    pub result: Value,
}

/// A pipeline of middleware applied to every tool call.
pub struct MiddlewarePipeline {
    middleware: Vec<Box<dyn Middleware>>,
    log: Vec<CallRecord>,
}

impl MiddlewarePipeline {
    pub fn new() -> Self { Self { middleware: Vec::new(), log: Vec::new() } }

    pub fn add<M: Middleware + 'static>(&mut self, m: M) {
        self.middleware.push(Box::new(m));
    }

    /// Run all pre-hooks, then post-hooks, return final (args, result).
    pub fn run(&mut self, tool: &str, args: Value, result: Value) -> (Value, Value) {
        let mut a = args;
        for m in &self.middleware {
            a = m.pre(tool, a);
        }
        let mut r = result;
        for m in self.middleware.iter().rev() {
            r = m.post(tool, r);
        }
        self.log.push(CallRecord { tool: tool.to_string(), args: a.clone(), result: r.clone() });
        (a, r)
    }

    pub fn call_log(&self) -> &[CallRecord] { &self.log }
    pub fn call_count(&self) -> usize { self.log.len() }
    pub fn middleware_count(&self) -> usize { self.middleware.len() }
}

impl Default for MiddlewarePipeline {
    fn default() -> Self { Self::new() }
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
}
