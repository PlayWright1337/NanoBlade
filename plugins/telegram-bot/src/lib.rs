wit_bindgen::generate!({
    path: "../../wit",
    world: "nexus-plugin",
});

use crate::nexus::runtime::host_log;
use crate::nexus::runtime::http_types::HttpHeader;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    message_id: i64,
    text: Option<String>,
    chat: TelegramChat,
    from: Option<TelegramUser>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
    #[serde(default)]
    r#type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramUser {
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Debug, Serialize)]
struct BotReply {
    ok: bool,
    update_id: Option<i64>,
    chat_id: Option<i64>,
    reply_text: String,
    command: String,
    note: &'static str,
}

struct TelegramBot;

impl Guest for TelegramBot {
    fn handle_request(request: HttpRequest) -> HttpResponse {
        host_log::write(
            host_log::LogLevel::Info,
            "telegram-bot plugin received webhook request",
        );

        let body_text = String::from_utf8_lossy(&request.body).to_string();
        let parsed = serde_json::from_str::<TelegramUpdate>(&body_text);

        let (status, payload) = match parsed {
            Ok(update) => {
                let reply = reply_for_update(update);
                (200, reply)
            }
            Err(error) => {
                host_log::write(
                    host_log::LogLevel::Warn,
                    "telegram-bot failed to parse incoming JSON body",
                );

                (
                    400,
                    BotReply {
                        ok: false,
                        update_id: None,
                        chat_id: None,
                        reply_text: format!("invalid telegram update payload: {error}"),
                        command: "invalid".into(),
                        note: "This plugin is a webhook processor. It does not call Telegram API directly.",
                    },
                )
            }
        };

        HttpResponse {
            status,
            headers: vec![
                HttpHeader {
                    name: "content-type".into(),
                    value: "application/json; charset=utf-8".into(),
                },
                HttpHeader {
                    name: "x-plugin".into(),
                    value: "telegram-bot".into(),
                },
            ],
            body: serde_json::to_vec(&payload).unwrap_or_else(|_| b"{\"ok\":false}".to_vec()),
        }
    }
}

fn reply_for_update(update: TelegramUpdate) -> BotReply {
    let Some(message) = update.message else {
        return BotReply {
            ok: true,
            update_id: Some(update.update_id),
            chat_id: None,
            reply_text: "update received, but there is no message payload".into(),
            command: "empty".into(),
            note: "This plugin is a webhook processor. It does not call Telegram API directly.",
        };
    };

    let text = message.text.unwrap_or_default();
    let name = message
        .from
        .as_ref()
        .and_then(|user| user.first_name.clone().or(user.username.clone()))
        .unwrap_or_else(|| "there".into());

    let (command, reply_text) = if text.starts_with("/start") {
        (
            "start".to_string(),
            format!(
                "Hi, {name}. I am a Telegram webhook plugin running inside Nexus Runtime."
            ),
        )
    } else if text.starts_with("/help") {
        (
            "help".to_string(),
            "Supported commands: /start, /help, /echo <text>".to_string(),
        )
    } else if let Some(rest) = text.strip_prefix("/echo ") {
        ("echo".to_string(), rest.to_string())
    } else if text.is_empty() {
        (
            "empty-text".to_string(),
            "Message received, but text is empty.".to_string(),
        )
    } else {
        (
            "fallback".to_string(),
            format!("You said: {text}"),
        )
    };

    host_log::write(
        host_log::LogLevel::Info,
        &format!(
            "telegram-bot command={} chat_id={} message_id={} chat_type={}",
            command,
            message.chat.id,
            message.message_id,
            message.chat.r#type.unwrap_or_else(|| "unknown".into())
        ),
    );

    BotReply {
        ok: true,
        update_id: Some(update.update_id),
        chat_id: Some(message.chat.id),
        reply_text,
        command,
        note: "This plugin is a webhook processor. It does not call Telegram API directly.",
    }
}

export!(TelegramBot);
