wit_bindgen::generate!({
    path: "../../wit",
    world: "nexus-plugin",
});

use crate::nexus::runtime::host_log;
use crate::nexus::runtime::http_types::HttpHeader;

struct HelloWorld;

impl Guest for HelloWorld {
    fn handle_request(request: HttpRequest) -> HttpResponse {
        host_log::write(
            host_log::LogLevel::Info,
            "hello-world plugin received a request",
        );

        let body = format!(
            "Hello from Nexus Runtime!\nmethod={}\nuri={}\nheaders={}\nbody_len={}\n",
            request.method,
            request.uri,
            request.headers.len(),
            request.body.len()
        );

        HttpResponse {
            status: 200,
            headers: vec![
                HttpHeader {
                    name: "content-type".into(),
                    value: "text/plain; charset=utf-8".into(),
                },
                HttpHeader {
                    name: "x-plugin".into(),
                    value: "hello-world".into(),
                },
            ],
            body: body.into_bytes(),
        }
    }
}

export!(HelloWorld);

