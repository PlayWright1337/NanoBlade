wit_bindgen::generate!({
    path: "../../wit",
    world: "nexus-plugin",
});

use crate::nexus::runtime::host_log;
use crate::nexus::runtime::http_types::HttpHeader;

struct HttpEcho;

impl Guest for HttpEcho {
    fn handle_request(request: HttpRequest) -> HttpResponse {
        let log_line = format!("{} {}", request.method, request.uri);
        host_log::write(host_log::LogLevel::Info, &log_line);

        let mut headers = vec![
            HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            },
            HttpHeader {
                name: "x-runtime".into(),
                value: "nexus-runtime".into(),
            },
        ];

        headers.extend(request.headers.iter().cloned());

        let body = format!(
            "{{\"method\":\"{}\",\"uri\":\"{}\",\"body_len\":{},\"header_count\":{}}}",
            request.method,
            request.uri,
            request.body.len(),
            request.headers.len()
        );

        HttpResponse {
            status: 200,
            headers,
            body: body.into_bytes(),
        }
    }
}

export!(HttpEcho);
