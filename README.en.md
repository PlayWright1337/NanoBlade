# NanoBlade

Isolated edge runtime for executing user-provided WebAssembly plugins with `Wasmtime` and the WIT Component Model.

`Nexus Runtime` accepts a `.wasm` component, creates a dedicated sandbox for it, limits CPU, memory, and execution time, exposes only explicitly allowed host APIs, and returns a structured HTTP-like response back to the host. It is a foundation for edge functions, plugin execution, sandboxed extensions, and multi-tenant serverless workloads.

## Table of Contents

- [What It Is](#what-it-is)
- [Key Properties](#key-properties)
- [Why It Is Safe](#why-it-is-safe)
- [Architecture](#architecture)
- [Project Structure](#project-structure)
- [WIT Contract](#wit-contract)
- [Request Lifecycle](#request-lifecycle)
- [Requirements](#requirements)
- [Quick Start](#quick-start)
- [Build and Run](#build-and-run)
- [Writing Plugins](#writing-plugins)
- [Plugins in Rust](#plugins-in-rust)
- [Plugins in Go and Other Languages](#plugins-in-go-and-other-languages)
- [Security Model and Limits](#security-model-and-limits)
- [Runtime Configuration](#runtime-configuration)
- [Logging and Observability](#logging-and-observability)
- [Current Limitations](#current-limitations)
- [Troubleshooting](#troubleshooting)
- [Roadmap](#roadmap)

## What It Is

`Nexus Runtime` is a Rust host application that:

- compiles and loads a WebAssembly Component;
- instantiates it inside an isolated `Store`;
- applies explicit resource limits;
- calls the exported `handle-request` function;
- receives an `http-response` back;
- keeps plugin failures isolated from the main process.

The current implementation focuses on a narrow request path: "take an HTTP-like request -> send it into a plugin -> receive an HTTP-like response". That narrow contract is deliberate. A smaller API surface is easier to secure, reason about, and evolve.

## Key Properties

- **Per-call isolation**: every invocation gets a fresh `Store`.
- **Fuel metering**: CPU budget is enforced via Wasmtime fuel consumption.
- **Memory limits**: linear memory size and wasm resource counts are restricted.
- **Execution timeout**: guest calls are bounded by wall-clock timeout.
- **Typed interface**: the contract is defined in WIT rather than ad-hoc JSON or raw ABI.
- **Async execution**: host and guest calls run through `tokio`.
- **Controlled imports only**: the guest only sees what the host explicitly adds to the linker.
- **Structured logging**: plugins can emit logs through `host-log.write`.

## Why It Is Safe

Safety here comes from multiple layers, not a single mechanism.

### 1. Execution Isolation

Plugins do not run as native shared libraries and do not get direct access to the host process memory. They run as WebAssembly Components inside Wasmtime.

That means:

- no arbitrary Rust/C ABI calls into the host;
- no direct filesystem access unless you explicitly expose it;
- no socket access unless you explicitly expose it;
- no shared heap between guest and host;
- a guest trap should not crash the main process.

### 2. Capability-Based Surface

A plugin can only use imports that exist in WIT and are explicitly wired into the linker. In the current project that is:

- `host-log.write`
- the minimal WASI P2 surface needed for `wasm32-wasip2` guest components

If an API is not defined in WIT and not registered in the linker, the plugin cannot use it.

### 3. CPU Limiting

Each invocation runs with a fixed fuel budget. When fuel is exhausted, Wasmtime traps and stops execution. This protects the host from infinite loops and overly expensive computation.

### 4. Memory Limiting

Each `Store` gets `StoreLimits`, which bound:

- linear memory size;
- instance count;
- table count;
- memory count.

This prevents uncontrolled wasm-side resource growth.

### 5. Time Limiting

Even if the guest does not exhaust fuel quickly enough, the call is additionally wrapped in `tokio::time::timeout`, giving you a wall-clock deadline.

### 6. Failure Containment

Component compilation errors, linker errors, guest traps, fuel exhaustion, and timeouts are returned as invocation-scoped failures. They do not terminate the whole host process.

## Architecture

The project has three core parts.

### `Runtime`

`Runtime` owns the shared `wasmtime::Engine` and execution policy:

- enables the Component Model;
- enables fuel metering;
- stores runtime limits;
- compiles plugin bytes into a `Component`;
- creates an isolated `Isolate`.

### `Isolate`

`Isolate` is the object that executes requests against an already compiled component. For every call it:

1. creates a new `Store`;
2. applies memory and resource limits;
3. injects fuel;
4. builds a `Linker`;
5. registers host imports;
6. instantiates the guest;
7. calls `handle-request`.

### `HostState`

`HostState` lives inside `Store` and contains:

- `StoreLimits`
- `WasiCtx`
- `ResourceTable`

It also implements the host traits generated from WIT.

## Project Structure

```text
.
├── Cargo.toml
├── Cargo.lock
├── README.md
├── README.ru.md
├── README.en.md
├── src/
│   └── main.rs
├── wit/
│   └── nexus-runtime.wit
└── plugins/
    └── http-echo/
        ├── Cargo.toml
        └── src/
            └── lib.rs
```

## WIT Contract

The contract lives in [wit/nexus-runtime.wit](./wit/nexus-runtime.wit).

It defines:

- the `host-log` logging interface;
- `http-header`, `http-request`, and `http-response` types;
- the `nexus-plugin` world with the `handle-request` export.

Simplified shape:

```wit
world nexus-plugin {
    import host-log;
    use http-types.{http-request, http-response};
    export handle-request: func(request: http-request) -> http-response;
}
```

## Request Lifecycle

1. The host reads the `.wasm` path from the CLI.
2. `Runtime::spawn_isolate` compiles bytes into `wasmtime::component::Component`.
3. `Isolate::handle` creates a new `Store<HostState>`.
4. Memory and resource limits are applied to that `Store`.
5. Fuel is injected.
6. The `Linker` is populated with WASI P2 imports and explicit host imports.
7. The component is instantiated through `NexusPlugin::instantiate_async`.
8. The host calls `call_handle_request`.
9. The plugin returns `HttpResponse`.
10. The host lifts the response back into Rust data structures.

## Requirements

- stable Rust toolchain
- `cargo`
- `wasm32-wasip2` target for guest components

Install the target:

```powershell
rustup target add wasm32-wasip2
```

## Quick Start

### 1. Check the host

```powershell
cargo check
```

### 2. Build the example plugin

```powershell
cargo build --manifest-path .\plugins\http-echo\Cargo.toml --target wasm32-wasip2 --release
```

### 3. Run the runtime

```powershell
cargo run -- .\plugins\http-echo\target\wasm32-wasip2\release\http_echo.wasm
```

Expected output:

```text
status: 200
headers:
  content-type: application/json
  x-runtime: nexus-runtime
  host: example.internal
  x-request-id: req-0001
body:
{"method":"GET","uri":"/edge/health","body_len":15,"header_count":2}
```

## Build and Run

### Host

```powershell
cargo build
cargo run -- .\path\to\plugin.wasm
```

### Example plugin

```powershell
cargo check --manifest-path .\plugins\http-echo\Cargo.toml --target wasm32-wasip2
cargo build --manifest-path .\plugins\http-echo\Cargo.toml --target wasm32-wasip2 --release
```

## Example Plugins

The repository now includes three small reference plugins:

- `http-echo`: returns request metadata as JSON.
- `hello-world`: returns a plain-text greeting and basic request info.
- `telegram-bot`: parses an incoming Telegram webhook update and returns a JSON reply plan.

### Run `hello-world`

```powershell
cargo build --manifest-path .\plugins\hello-world\Cargo.toml --target wasm32-wasip2 --release
cargo run -- .\plugins\hello-world\target\wasm32-wasip2\release\hello_world.wasm
```

Expected behavior:

```text
status: 200
content-type: text/plain; charset=utf-8
x-plugin: hello-world
```

### Run `telegram-bot`

```powershell
cargo build --manifest-path .\plugins\telegram-bot\Cargo.toml --target wasm32-wasip2 --release
cargo run -- .\plugins\telegram-bot\target\wasm32-wasip2\release\telegram_bot.wasm
```

Important note:

- `telegram-bot` is a webhook-style plugin, not a full Telegram API client.
- It expects a Telegram update JSON payload in the request body.
- The current demo CLI sends a fixed sample body, so this example will return `400` until you feed it a real Telegram-style JSON payload.

## Writing Plugins

A `Nexus Runtime` plugin is a WebAssembly Component that:

- builds to `.wasm`;
- implements the `nexus-plugin` WIT world;
- exports `handle-request`;
- optionally calls `host-log.write`.

Minimum requirements:

1. It must generate bindings from [wit/nexus-runtime.wit](./wit/nexus-runtime.wit).
2. It must build to a format compatible with the WebAssembly Component Model.
3. It must export `handle-request`.

## Plugins in Rust

Rust is currently the most direct and mature path for this repository.

### Why Rust

- strong `wit-bindgen` integration;
- straightforward `wasm32-wasip2` build path;
- predictable Component binding generation.

### Rust plugin template

```rust
wit_bindgen::generate!({
    path: "../../wit",
    world: "nexus-plugin",
});

use crate::nexus::runtime::host_log;
use crate::nexus::runtime::http_types::HttpHeader;

struct MyPlugin;

impl Guest for MyPlugin {
    fn handle_request(request: HttpRequest) -> HttpResponse {
        host_log::write(host_log::LogLevel::Info, "plugin invoked");

        HttpResponse {
            status: 200,
            headers: vec![
                HttpHeader {
                    name: "content-type".into(),
                    value: "text/plain".into(),
                },
            ],
            body: b"hello from plugin".to_vec(),
        }
    }
}

export!(MyPlugin);
```

### Minimal `Cargo.toml`

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.54.0"
```

### Build a Rust plugin

```powershell
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release
```

## Plugins in Go and Other Languages

Yes, architecturally plugins do not have to be written in Rust.

What matters is not the language itself but whether the language and toolchain can:

- compile to WebAssembly;
- understand WIT or generate compatible bindings;
- emit a Component Model compatible artifact, not just a plain core wasm module.

What can be stated honestly today:

- the Rust path is implemented and verified in this repository;
- the runtime is not inherently Rust-only;
- Go, TinyGo, and other languages are possible if their toolchains can produce a component-compatible artifact for this WIT contract.

Checklist for a non-Rust plugin:

1. The toolchain supports WIT.
2. The toolchain supports the Component Model.
3. The plugin exports `handle-request`.
4. The resulting `.wasm` instantiates in Wasmtime without missing imports or type mismatches.

## Security Model and Limits

### CPU limit

The runtime uses `fuel_per_request`. For every invocation the host injects fuel into the `Store` via `set_fuel`.

### Memory limit

`StoreLimitsBuilder` sets:

- `memory_size`
- `instances`
- `tables`
- `memories`

### Wall-clock timeout

The guest call is wrapped in:

```rust
tokio::time::timeout(...)
```

### Trap handling

Guest failures are returned as `Result::Err` instead of being treated as process-fatal.

### Restricted host surface

The guest sees only:

- `host-log`
- the WASI P2 surface required to execute a `wasm32-wasip2` component

## Runtime Configuration

The current policy is defined in `RuntimeConfig` in [src/main.rs](./src/main.rs).

| Field | Purpose | Current value |
| --- | --- | --- |
| `fuel_per_request` | CPU instruction budget | `250_000` |
| `max_memory_bytes` | linear memory limit | `8 MiB` |
| `max_instances` | max instances per store | `4` |
| `max_tables` | max tables | `8` |
| `max_memories` | max memories | `4` |
| `execution_timeout` | wall-clock timeout | `50 ms` |

## Logging and Observability

Plugins can emit logs through:

```rust
host_log::write(host_log::LogLevel::Info, "message");
```

The host receives that in `HostState::write` and forwards it into `tracing`.

## Current Limitations

- currently this is a single binary with a demo CLI;
- `main.rs` has not yet been split into dedicated modules;
- there is no HTTP server layer above the runtime;
- there is no pre-instantiated component cache;
- there is no plugin policy engine yet;
- there is no non-Rust plugin example in the repository.

## Troubleshooting

### `the wasm32-wasip2 target may not be installed`

```powershell
rustup target add wasm32-wasip2
```

### `component imports instance ... but a matching implementation was not found in the linker`

Check:

- which world the plugin actually exports;
- which WASI imports the guest needs;
- whether the WIT contract matches between host and plugin;
- whether the required imports were added to the linker.

### `resource limit exceeded`

The problem is usually one of:

- `max_instances`
- `max_tables`
- `max_memories`
- `max_memory_bytes`

### `guest execution exceeded ...`

Common causes are expensive logic, infinite loops, or an overly aggressive timeout.

## Roadmap

1. Split the host into dedicated modules.
2. Add unit and integration tests.
3. Add a component cache.
4. Add an HTTP server layer.
5. Introduce a plugin policy engine.
6. Add a plugin manifest and capability model.
7. Add a non-Rust plugin example.

## Summary

`NanoBlade Runtime` is safe not because the host blindly trusts plugins, but because trust is minimized:

- small WIT surface;
- wasm sandbox;
- fuel limits;
- memory limits;
- timeout;
- localized trap handling.
