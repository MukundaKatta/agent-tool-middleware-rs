# agent-tool-middleware

A small, dependency-light **middleware pipeline for LLM agent tool calls** in Rust.

It lets you wrap every tool invocation an agent makes with composable hooks that
can rewrite the call arguments *before* the tool runs and the result *after* it
returns — for logging, argument injection, redaction, validation, and similar
cross-cutting concerns — without changing the tools themselves.

## Concept

Tool calls flow through an ordered stack of middleware:

- **`pre(tool, args) -> args`** runs in registration order and can transform the
  arguments before the tool executes.
- **`post(tool, result) -> result`** runs in reverse order and can transform the
  result after the tool returns.

Every call is recorded in the pipeline's call log so you can inspect what an
agent actually did.

## Installation

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
agent-tool-middleware = { git = "https://github.com/MukundaKatta/agent-tool-middleware-rs" }
serde_json = "1"
```

## Usage

```rust
use agent_tool_middleware::{MiddlewarePipeline, InjectFieldMiddleware};
use serde_json::json;

let mut pipe = MiddlewarePipeline::new();

// Inject a field into the args of every tool call.
pipe.add(InjectFieldMiddleware::new("version", json!("1.0")));

// Run a tool call through the pipeline.
let (args, result) = pipe.run("search", json!({ "q": "rust" }), json!({ "hits": 5 }));

assert_eq!(args["q"], "rust");
assert_eq!(args["version"], "1.0");
assert_eq!(result["hits"], 5);

// Inspect what happened.
assert_eq!(pipe.call_count(), 1);
assert_eq!(pipe.call_log()[0].tool, "search");
```

## Writing custom middleware

Implement the `Middleware` trait. Both hooks have sensible pass-through defaults,
so you only override the side you care about:

```rust
use agent_tool_middleware::Middleware;
use serde_json::Value;

struct RedactSecrets;

impl Middleware for RedactSecrets {
    fn name(&self) -> &str { "redact" }

    fn pre(&self, _tool: &str, mut args: Value) -> Value {
        if let Some(obj) = args.as_object_mut() {
            obj.remove("api_key");
        }
        args
    }
}
```

## API overview

- `Middleware` — trait with `name`, `pre` (args transform), and `post` (result transform).
- `MiddlewarePipeline` — registers middleware via `add`, runs a call via `run`,
  and exposes `call_log`, `call_count`, and `middleware_count`.
- `InjectFieldMiddleware` — adds a fixed field to every object-shaped args payload.
- `LogMiddleware` — records calls passing through the pipeline.
- `CallRecord` — a single logged `{ tool, args, result }` entry.

## Tech stack

- **Language:** Rust (edition 2021)
- **Dependencies:** [`serde_json`](https://crates.io/crates/serde_json) for the
  dynamic JSON `Value` type used to carry tool arguments and results.

## Development

```bash
cargo build      # compile the crate
cargo test       # run the unit tests
cargo clippy     # lint
```

## License

Licensed under the MIT License.
