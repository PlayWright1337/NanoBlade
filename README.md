# Nexus Runtime

Choose documentation language:

- [English README](./README.en.md)
- [Русская версия](./README.ru.md)

---

Кратко:

- sandboxed edge runtime for WebAssembly Components
- typed plugin contract via WIT
- resource isolation with fuel, memory and timeout limits
- Rust plugin example included

Quick start:

```powershell
rustup target add wasm32-wasip2
cargo build --manifest-path .\plugins\http-echo\Cargo.toml --target wasm32-wasip2 --release
cargo run -- .\plugins\http-echo\target\wasm32-wasip2\release\http_echo.wasm
```

