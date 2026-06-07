# agent-tool-middleware

[![CI](https://github.com/MukundaKatta/agent-tool-middleware-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/MukundaKatta/agent-tool-middleware-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)

A tiny, dependency-light **middleware pipeline for LLM agent tool calls** in Rust.

When an agent calls a tool you often want to do work *around* the call —
inject default arguments, redact secrets out of results, enforce limits, or
log every invocation for auditing. This crate lets you express each of those
concerns as a small [`Middleware`] and compose them into an ordered
[`MiddlewarePipeline`].

The pipeline uses the classic **onion model** also seen in HTTP middleware
stacks:

- `pre` hooks run in **registration order** (outermost first) and transform
  the call *arguments* before the tool runs.
- `post` hooks run in **reverse order** and transform the *result* after the
  tool runs.

The only runtime dependency is [`serde_json`](https://crates.io/crates/serde_json),
so payloads are plain `serde_json::Value`s.

## Install

This crate is not published to crates.io. Add it as a git dependency:

```toml
[dependencies]
agent-tool-middleware = { git = "https://github.com/MukundaKatta/agent-tool-middleware-rs" }
serde_json = "1"
```

Or vendor it as a path dependency:

```toml
[dependencies]
agent-tool-middleware = { path = "../agent-tool-middleware-rs" }
```

## Usage

```rust
use agent_tool_middleware::{
    InjectFieldMiddleware, LogMiddleware, MiddlewarePipeline, RedactFieldMiddleware,
};
use serde_json::json;

fn main() {
    // Keep a handle so we can read the audit log after the pipeline owns a clone.
    let logger = LogMiddleware::new();

    let mut pipe = MiddlewarePipeline::new();
    pipe.add(InjectFieldMiddleware::new("api_version", json!("2024-01")));
    pipe.add(RedactFieldMiddleware::new("token"));
    pipe.add(logger.handle());

    // `args` is what your real tool would receive; `result` is what it returned.
    let (args, result) = pipe.run(
        "search",
        json!({ "q": "rust" }),
        json!({ "hits": 5, "token": "super-secret" }),
    );

    // pre: the default field was injected into the arguments.
    assert_eq!(args["api_version"], "2024-01");

    // post: the secret was scrubbed from the result.
    assert_eq!(result["token"], "[redacted]");
    assert_eq!(result["hits"], 5);

    // The pipeline keeps its own audit log; LogMiddleware keeps a shared one.
    assert_eq!(pipe.call_count(), 1);
    assert_eq!(logger.entries()[0].tool, "search");
}
```

## API

### `trait Middleware`

| Item | Description |
| --- | --- |
| `fn name(&self) -> &str` | A short, stable identifier for the middleware. |
| `fn pre(&self, tool: &str, args: Value) -> Value` | Transform arguments before the call (defaults to identity). |
| `fn post(&self, tool: &str, result: Value) -> Value` | Transform the result after the call (defaults to identity). |

Implement only the hook you need; the other defaults to a pass-through.

### `struct MiddlewarePipeline`

| Method | Description |
| --- | --- |
| `new()` / `default()` | Create an empty pipeline. |
| `add<M: Middleware + 'static>(&mut self, m: M)` | Append a middleware to the stack. |
| `run(&mut self, tool, args, result) -> (Value, Value)` | Run all `pre` then all `post` hooks, record the call, and return the final pair. |
| `call_log() -> &[CallRecord]` | All recorded calls, oldest first. |
| `call_count() -> usize` | Number of recorded calls. |
| `middleware_count() -> usize` | Number of registered middleware. |
| `middleware_names() -> Vec<&str>` | The `name()` of each middleware, in order. |
| `clear_log(&mut self)` | Discard the call log without removing middleware. |

### Built-in middleware

- **`InjectFieldMiddleware::new(field, value)`** — inserts/overwrites a field on
  object-shaped argument payloads in `pre`. Non-object payloads pass through.
- **`RedactFieldMiddleware::new(field)`** / **`with_placeholder(field, value)`** —
  replaces `field` in object-shaped *results* with `"[redacted]"` (or a custom
  placeholder) in `post`. Absent fields and non-object results are untouched.
- **`LogMiddleware`** — records every call it observes. Its log lives behind an
  `Arc<Mutex<..>>`; call [`handle()`](#) to add a shared clone to the pipeline
  while keeping the original for inspection via `entries()`, `len()`,
  `is_empty()`, and `clear()`.

### `struct CallRecord`

A record of one call: `tool: String`, `args: Value`, `result: Value`.

## Development

```bash
cargo build
cargo test          # unit + doc tests
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## License

MIT
