use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use bytes::Bytes;
use tokio::time::timeout;
use tracing::{Level, debug, error, info, warn};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "nexus-plugin",
        imports: { default: async | trappable },
        exports: { default: async },
    });
}

mod model {
    use super::*;
    use crate::bindings::nexus::runtime::http_types::{
        HttpHeader as BindingHeader, HttpRequest as BindingRequest, HttpResponse as BindingResponse,
    };

    #[derive(Clone, Debug)]
    pub struct Header {
        pub name: String,
        pub value: String,
    }

    #[derive(Clone, Debug)]
    pub struct HttpRequest {
        pub method: String,
        pub uri: String,
        pub headers: Vec<Header>,
        pub body: Bytes,
    }

    #[derive(Clone, Debug)]
    pub struct HttpResponse {
        pub status: u16,
        pub headers: Vec<Header>,
        pub body: Bytes,
    }

    impl HttpRequest {
        pub fn new(
            method: impl Into<String>,
            uri: impl Into<String>,
            headers: Vec<Header>,
            body: impl Into<Bytes>,
        ) -> Self {
            Self {
                method: method.into(),
                uri: uri.into(),
                headers,
                body: body.into(),
            }
        }
    }

    impl From<Header> for BindingHeader {
        fn from(value: Header) -> Self {
            Self {
                name: value.name,
                value: value.value,
            }
        }
    }

    impl From<BindingHeader> for Header {
        fn from(value: BindingHeader) -> Self {
            Self {
                name: value.name,
                value: value.value,
            }
        }
    }

    impl From<HttpRequest> for BindingRequest {
        fn from(value: HttpRequest) -> Self {
            Self {
                method: value.method,
                uri: value.uri,
                headers: value.headers.into_iter().map(Into::into).collect(),
                body: value.body.to_vec(),
            }
        }
    }

    impl From<BindingResponse> for HttpResponse {
        fn from(value: BindingResponse) -> Self {
            Self {
                status: value.status,
                headers: value.headers.into_iter().map(Into::into).collect(),
                body: Bytes::from(value.body),
            }
        }
    }
}

mod runtime {
    use super::*;

    const DEFAULT_FUEL: u64 = 250_000;
    const DEFAULT_MAX_MEMORY_BYTES: usize = 8 * 1024 * 1024;
    const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_millis(50);

    #[derive(Clone, Debug)]
    pub struct RuntimeConfig {
        pub fuel_per_request: u64,
        pub max_memory_bytes: usize,
        pub max_instances: usize,
        pub max_tables: usize,
        pub max_memories: usize,
        pub execution_timeout: Duration,
    }

    impl Default for RuntimeConfig {
        fn default() -> Self {
            Self {
                fuel_per_request: DEFAULT_FUEL,
                max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
                max_instances: 4,
                max_tables: 8,
                max_memories: 4,
                execution_timeout: DEFAULT_EXECUTION_TIMEOUT,
            }
        }
    }

    #[derive(Debug)]
    pub struct Runtime {
        engine: Engine,
        config: RuntimeConfig,
    }

    impl Runtime {
        pub fn new(config: RuntimeConfig) -> Result<Self> {
            let mut engine_config = Config::new();
            engine_config.consume_fuel(true);
            engine_config.wasm_component_model(true);

            let engine = Engine::new(&engine_config)
                .map_err(|e| anyhow!("failed to build wasmtime engine: {e}"))?;

            Ok(Self { engine, config })
        }

        pub fn spawn_isolate(&self, wasm_bytes: Vec<u8>) -> Result<Isolate> {
            let component = Component::new(&self.engine, wasm_bytes)
                .map_err(|e| anyhow!("failed to compile component for isolate: {e}"))?;

            Ok(Isolate {
                engine: self.engine.clone(),
                component,
                config: self.config.clone(),
            })
        }
    }

    pub struct Isolate {
        engine: Engine,
        component: Component,
        config: RuntimeConfig,
    }

    impl Isolate {
        pub async fn handle(
            &self,
            request: model::HttpRequest,
        ) -> Result<model::HttpResponse> {
            let mut store = self.new_store();
            let linker = self.build_linker()?;

            let guest = bindings::NexusPlugin::instantiate_async(
                &mut store,
                &self.component,
                &linker,
            )
            .await
            .map_err(|e| anyhow!("failed to instantiate isolate: {e}"))?;

            let request = bindings::HttpRequest::from(request);

            let response = timeout(
                self.config.execution_timeout,
                guest.call_handle_request(&mut store, &request),
            )
            .await
            .map_err(|_| anyhow!("guest execution exceeded {:?}", self.config.execution_timeout))?
            .map_err(|e| anyhow!("guest trapped while handling request: {e}"))?;

            let remaining_fuel = store
                .get_fuel()
                .map_err(|e| anyhow!("failed to read remaining fuel after request: {e}"))?;

            debug!(
                fuel_remaining = remaining_fuel,
                fuel_consumed = self.config.fuel_per_request.saturating_sub(remaining_fuel),
                "request completed inside isolate"
            );

            Ok(response.into())
        }

        fn build_linker(&self) -> Result<Linker<HostState>> {
            let mut linker = Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_async(&mut linker)
                .map_err(|e| anyhow!("failed to wire WASI imports into linker: {e}"))?;
            bindings::NexusPlugin::add_to_linker::<_, wasmtime::component::HasSelf<HostState>>(
                &mut linker,
                |state: &mut HostState| state,
            )
            .map_err(|e| anyhow!("failed to wire host imports into linker: {e}"))?;
            Ok(linker)
        }

        fn new_store(&self) -> Store<HostState> {
            let limits = StoreLimitsBuilder::new()
                .memory_size(self.config.max_memory_bytes)
                .instances(self.config.max_instances)
                .tables(self.config.max_tables)
                .memories(self.config.max_memories)
                .build();

            let state = HostState {
                limits,
                wasi: WasiCtxBuilder::new().build(),
                table: ResourceTable::new(),
            };
            let mut store = Store::new(&self.engine, state);
            store.limiter(|state| &mut state.limits);
            store
                .set_fuel(self.config.fuel_per_request)
                .expect("fuel must be enabled on engine config");
            store
        }
    }

    pub struct HostState {
        limits: StoreLimits,
        wasi: WasiCtx,
        table: ResourceTable,
    }

    impl WasiView for HostState {
        fn ctx(&mut self) -> WasiCtxView<'_> {
            WasiCtxView {
                ctx: &mut self.wasi,
                table: &mut self.table,
            }
        }
    }

    impl bindings::nexus::runtime::http_types::Host for HostState {}

    impl bindings::nexus::runtime::host_log::Host for HostState {
        async fn write(
            &mut self,
            level: bindings::nexus::runtime::host_log::LogLevel,
            message: String,
        ) -> Result<(), wasmtime::Error> {
            match level {
                bindings::nexus::runtime::host_log::LogLevel::Trace => debug!("{message}"),
                bindings::nexus::runtime::host_log::LogLevel::Debug => debug!("{message}"),
                bindings::nexus::runtime::host_log::LogLevel::Info => info!("{message}"),
                bindings::nexus::runtime::host_log::LogLevel::Warn => warn!("{message}"),
                bindings::nexus::runtime::host_log::LogLevel::Error => error!("{message}"),
            }

            Ok(())
        }
    }
}

mod cli {
    use super::*;

    pub fn plugin_path() -> Result<PathBuf> {
        std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("usage: cargo run -- <path-to-plugin.wasm>"))
    }

    pub fn sample_request() -> model::HttpRequest {
        model::HttpRequest::new(
            "GET",
            "/edge/health",
            vec![
                model::Header {
                    name: "host".into(),
                    value: "example.internal".into(),
                },
                model::Header {
                    name: "x-request-id".into(),
                    value: "req-0001".into(),
                },
            ],
            Bytes::from_static(br#"{"ping":"pong"}"#),
        )
    }

    pub fn read_wasm(path: &Path) -> Result<Vec<u8>> {
        std::fs::read(path).with_context(|| format!("failed to read component {}", path.display()))
    }

    pub fn print_response(response: &model::HttpResponse) {
        println!("status: {}", response.status);
        println!("headers:");
        for header in &response.headers {
            println!("  {}: {}", header.name, header.value);
        }
        println!("body:\n{}", String::from_utf8_lossy(&response.body));
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,nexus_runtime=debug".into()),
        )
        .init();

    let plugin_path = cli::plugin_path()?;
    let wasm_bytes = cli::read_wasm(&plugin_path)?;

    let runtime = runtime::Runtime::new(runtime::RuntimeConfig::default())?;
    let isolate = runtime.spawn_isolate(wasm_bytes)?;
    let response = isolate.handle(cli::sample_request()).await?;

    cli::print_response(&response);

    Ok(())
}
