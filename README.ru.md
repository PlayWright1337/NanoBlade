# NanoBlade

Изолированный edge runtime для запуска пользовательских WebAssembly-плагинов через `Wasmtime` и WIT Component Model.

`Nexus Runtime` принимает `.wasm`-компонент, поднимает для него отдельную песочницу, ограничивает CPU, память и время исполнения, пробрасывает только явно разрешенные host API и возвращает структурированный HTTP-ответ обратно в хост. Это фундамент для edge-функций, plugin execution, sandboxed extensions и multi-tenant serverless workloads.

## Содержание

- [Что это](#что-это)
- [Ключевые свойства](#ключевые-свойства)
- [Почему это безопасно](#почему-это-безопасно)
- [Архитектура](#архитектура)
- [Структура проекта](#структура-проекта)
- [WIT-контракт](#wit-контракт)
- [Как выполняется запрос](#как-выполняется-запрос)
- [Требования](#требования)
- [Быстрый старт](#быстрый-старт)
- [Сборка и запуск](#сборка-и-запуск)
- [Написание плагинов](#написание-плагинов)
- [Плагины на Rust](#плагины-на-rust)
- [Плагины на Go и других языках](#плагины-на-go-и-других-языках)
- [Модель безопасности и ограничения](#модель-безопасности-и-ограничения)
- [Конфигурация рантайма](#конфигурация-рантайма)
- [Логи и наблюдаемость](#логи-и-наблюдаемость)
- [Текущие ограничения](#текущие-ограничения)
- [Troubleshooting](#troubleshooting)
- [Roadmap](#roadmap)

## Что это

`Nexus Runtime` это хостовое Rust-приложение, которое:

- компилирует и загружает WebAssembly Component;
- инстанцирует его в отдельном `Store`;
- выставляет лимиты на ресурсы;
- вызывает экспорт `handle-request`;
- получает обратно `http-response`;
- изолирует сбои плагина от основного процесса.

В текущей реализации рантайм ориентирован на сценарий "получить HTTP-подобный запрос -> передать в плагин -> получить HTTP-подобный ответ". Это намеренно узкий контракт: чем меньше поверхность API, тем проще обеспечить безопасность, стабильность и предсказуемое поведение.

## Ключевые свойства

- **Изоляция по вызову**: каждый вызов выполняется в новом `Store`, без повторного использования состояния между запросами.
- **Fuel metering**: CPU-бюджет задается через fuel consumption в Wasmtime.
- **Memory limits**: размер linear memory и число wasm-ресурсов ограничены.
- **Execution timeout**: вызов плагина обрывается по wall-clock timeout.
- **Typed interface**: контракт задан через WIT, а не через сырые ABI или ad-hoc JSON.
- **Async execution**: хост и вызов guest работают в `tokio`.
- **Controlled imports only**: плагин получает только то, что хост явно добавил в linker.
- **Structured logging**: плагин может логировать через `host-log.write`.

## Почему это безопасно

Безопасность здесь строится на нескольких слоях.

### 1. Изоляция исполнения

Плагин не выполняется как нативная динамическая библиотека и не получает прямого доступа к памяти процесса. Он исполняется как WebAssembly Component внутри Wasmtime.

Это означает:

- нет прямого вызова произвольного Rust/C ABI хоста;
- нет прямого доступа к файловой системе хоста;
- нет доступа к сокетам, если вы их не пробросили;
- нет разделяемого heap между guest и host;
- trap внутри плагина не должен валить основной процесс.

### 2. Capability-based surface

Плагин может использовать только те импорты, которые заданы в WIT и добавлены в linker. В текущем проекте это:

- `host-log.write`
- минимальный набор WASI P2, нужный для исполнения `wasm32-wasip2` компонентов

Если API не описан в WIT и не добавлен в linker, плагин не может им пользоваться.

### 3. Ограничение CPU

Вызов исполняется с фиксированным fuel budget. Когда budget исчерпан, Wasmtime останавливает выполнение с trap. Это защищает от бесконечных циклов и слишком дорогих вычислений.

### 4. Ограничение памяти

На каждый `Store` навешивается `StoreLimits`, который ограничивает:

- размер linear memory;
- количество инстансов;
- количество таблиц;
- количество memories.

### 5. Ограничение времени

Вызов дополнительно ограничивается `tokio::time::timeout`. Это дает второй защитный рубеж по wall-clock времени.

### 6. Сбои локализованы

Ошибка компиляции компонента, ошибка линковки, trap в guest, превышение fuel или timeout возвращаются как ошибка конкретного вызова. Это не приводит к аварийному завершению всего хоста.

## Архитектура

В проекте три главные части.

### `Runtime`

`Runtime` владеет общим `wasmtime::Engine` и политикой исполнения:

- включает Component Model;
- включает fuel metering;
- хранит runtime limits;
- компилирует байты плагина в `Component`;
- создает изолированный `Isolate`.

### `Isolate`

`Isolate` это объект, готовый выполнить запрос против уже скомпилированного компонента. На каждый вызов он:

1. создает новый `Store`;
2. выставляет memory/resource limits;
3. заливает fuel;
4. собирает `Linker`;
5. добавляет host imports;
6. инстанцирует guest;
7. вызывает `handle-request`.

### `HostState`

`HostState` живет внутри `Store` и содержит:

- `StoreLimits`
- `WasiCtx`
- `ResourceTable`

Он же реализует host traits, сгенерированные из WIT.

## Структура проекта

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

## WIT-контракт

Контракт находится в [wit/nexus-runtime.wit](./wit/nexus-runtime.wit).

Он описывает:

- интерфейс логирования `host-log`;
- типы `http-header`, `http-request`, `http-response`;
- world `nexus-plugin` с экспортом `handle-request`.

Упрощенная схема:

```wit
world nexus-plugin {
    import host-log;
    use http-types.{http-request, http-response};
    export handle-request: func(request: http-request) -> http-response;
}
```

## Как выполняется запрос

1. Хост читает путь к `.wasm` из CLI.
2. `Runtime::spawn_isolate` компилирует байты в `wasmtime::component::Component`.
3. `Isolate::handle` строит новый `Store<HostState>`.
4. Для `Store` задаются лимиты по памяти и числу ресурсов.
5. В `Store` заливается fuel budget.
6. В `Linker` добавляются WASI P2 imports и host imports.
7. Компонент инстанцируется через `NexusPlugin::instantiate_async`.
8. Хост вызывает `call_handle_request`.
9. Плагин возвращает `HttpResponse`.
10. Хост поднимает ответ обратно в Rust-модель.

## Требования

- Rust stable
- `cargo`
- target `wasm32-wasip2` для guest components

Установка target:

```powershell
rustup target add wasm32-wasip2
```

## Быстрый старт

### 1. Проверить хост

```powershell
cargo check
```

### 2. Собрать пример плагина

```powershell
cargo build --manifest-path .\plugins\http-echo\Cargo.toml --target wasm32-wasip2 --release
```

### 3. Запустить рантайм

```powershell
cargo run -- .\plugins\http-echo\target\wasm32-wasip2\release\http_echo.wasm
```

Ожидаемый результат:

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

## Сборка и запуск

### Хост

```powershell
cargo build
cargo run -- .\path\to\plugin.wasm
```

### Пример плагина

```powershell
cargo check --manifest-path .\plugins\http-echo\Cargo.toml --target wasm32-wasip2
cargo build --manifest-path .\plugins\http-echo\Cargo.toml --target wasm32-wasip2 --release
```

## Написание плагинов

Плагин для `Nexus Runtime` это WebAssembly Component, который:

- собирается в `.wasm`;
- реализует WIT world `nexus-plugin`;
- экспортирует функцию `handle-request`;
- при необходимости вызывает `host-log.write`.

Минимальные требования:

1. Плагин должен генерировать bindings из [wit/nexus-runtime.wit](./wit/nexus-runtime.wit).
2. Он должен собираться в формат, совместимый с WebAssembly Component Model.
3. Он должен экспортировать `handle-request`.

## Плагины на Rust

Rust сейчас самый прямой и зрелый путь для этого репозитория.

### Почему Rust

- хорошая интеграция с `wit-bindgen`;
- понятная сборка в `wasm32-wasip2`;
- предсказуемая генерация Component bindings.

### Шаблон Rust-плагина

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

### Минимальный `Cargo.toml`

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

### Сборка Rust-плагина

```powershell
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release
```

## Плагины на Go и других языках

Да, архитектурно плагины можно писать не только на Rust.

Нужно, чтобы язык и toolchain умели:

- собираться в WebAssembly;
- поддерживать WIT или совместимую генерацию bindings;
- выпускать component-compatible артефакт, а не просто core wasm модуль.

Что можно честно обещать сейчас:

- путь для Rust показан и проверен;
- рантайм не привязан к Rust-плагинам;
- Go, TinyGo и другие языки возможны, если их toolchain умеет собрать совместимый Component Model артефакт по этому WIT-контракту.

Проверочный чеклист для не-Rust плагинов:

1. Toolchain поддерживает WIT.
2. Toolchain поддерживает Component Model.
3. Экспортируется `handle-request`.
4. Итоговый `.wasm` инстанцируется в Wasmtime без missing imports и type mismatch.

## Модель безопасности и ограничения

### CPU limit

Используется `fuel_per_request`. На каждый вызов budget заливается в `Store` через `set_fuel`.

### Memory limit

Через `StoreLimitsBuilder` задаются:

- `memory_size`
- `instances`
- `tables`
- `memories`

### Wall-clock timeout

Вызов `handle-request` обернут в:

```rust
tokio::time::timeout(...)
```

### Trap handling

Ошибки гостя не считаются фатальными для процесса. Они возвращаются как `Result::Err`.

### Ограниченный host surface

Плагин видит только:

- `host-log`
- то, что необходимо для исполнения `wasm32-wasip2` компонента

## Конфигурация рантайма

Текущая policy задается в `RuntimeConfig` в [src/main.rs](./src/main.rs).

| Поле | Назначение | Текущее значение |
| --- | --- | --- |
| `fuel_per_request` | бюджет CPU-инструкций | `250_000` |
| `max_memory_bytes` | лимит linear memory | `8 MiB` |
| `max_instances` | максимум instances в store | `4` |
| `max_tables` | максимум tables | `8` |
| `max_memories` | максимум memories | `4` |
| `execution_timeout` | wall-clock timeout | `50 ms` |

## Логи и наблюдаемость

Плагин может писать лог через:

```rust
host_log::write(host_log::LogLevel::Info, "message");
```

Хост принимает это в `HostState::write` и переводит в `tracing`.

## Текущие ограничения

- пока это один бинарник с demo CLI;
- `main.rs` еще не разнесен по отдельным модулям;
- нет HTTP-сервера поверх runtime;
- нет кэша pre-instantiated components;
- нет policy engine для прав плагинов;
- нет примера плагина не на Rust.

## Примерные плагины

В репозитории теперь есть три маленьких reference-плагина:

- `http-echo`: возвращает метаданные запроса в JSON.
- `hello-world`: возвращает plain-text приветствие и базовую информацию о запросе.
- `telegram-bot`: разбирает входящий Telegram webhook update и возвращает JSON с ответом бота.

### Запуск `hello-world`

```powershell
cargo build --manifest-path .\plugins\hello-world\Cargo.toml --target wasm32-wasip2 --release
cargo run -- .\plugins\hello-world\target\wasm32-wasip2\release\hello_world.wasm
```

Ожидаемое поведение:

```text
status: 200
content-type: text/plain; charset=utf-8
x-plugin: hello-world
```

### Запуск `telegram-bot`

```powershell
cargo build --manifest-path .\plugins\telegram-bot\Cargo.toml --target wasm32-wasip2 --release
cargo run -- .\plugins\telegram-bot\target\wasm32-wasip2\release\telegram_bot.wasm
```

Важно:

- `telegram-bot` сейчас сделан как webhook-плагин, а не как полноценный клиент Telegram Bot API.
- Он ожидает в body JSON в формате Telegram update.
- Текущий demo CLI отправляет фиксированное тестовое тело, поэтому этот пример вернет `400`, пока в runtime не будет передан реальный Telegram-style JSON.

## Troubleshooting

### `the wasm32-wasip2 target may not be installed`

```powershell
rustup target add wasm32-wasip2
```

### `component imports instance ... but a matching implementation was not found in the linker`

Проверьте:

- какой world реально экспортирует плагин;
- какие WASI imports нужны guest;
- совпадает ли WIT контракт между host и plugin;
- добавлены ли нужные imports в linker.

### `resource limit exceeded`

Обычно проблема в одном из лимитов:

- `max_instances`
- `max_tables`
- `max_memories`
- `max_memory_bytes`

### `guest execution exceeded ...`

Обычно причина в тяжелой логике, бесконечном цикле или слишком агрессивном timeout.

## Roadmap

1. Разнести хост на отдельные модули.
2. Добавить unit/integration tests.
3. Сделать component cache.
4. Добавить HTTP server layer.
5. Ввести policy engine для прав плагинов.
6. Добавить plugin manifest и capability model.
7. Подготовить пример плагина не на Rust.

## Итог

`NanoBlade` безопасен не потому, что хост доверяет плагину, а потому что доверие минимально:

- маленький WIT surface;
- wasm sandbox;
- fuel limits;
- memory limits;
- timeout;
- локализованная обработка trap.
