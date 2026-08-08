use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use reqwest::Client as HttpClient;
use serde_json::json;
use serenity::async_trait;
use serenity::builder::{CreateAttachment, CreateMessage};
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;
use tokio::sync::Mutex;

struct ApiKeyManager {
    keys: Vec<String>,
    current_index: usize,
}

impl ApiKeyManager {
    fn new(keys: Vec<String>) -> Self {
        Self {
            keys,
            current_index: 0,
        }
    }

    fn get_current_key(&self) -> Option<String> {
        self.keys.get(self.current_index).cloned()
    }

    fn rotate(&mut self) {
        if self.keys.is_empty() {
            return;
        }
        self.current_index = (self.current_index + 1) % self.keys.len();
    }

    fn len(&self) -> usize {
        self.keys.len()
    }
}

struct AppState {
    api_manager: Mutex<ApiKeyManager>,
    http_client: HttpClient,
}

struct Handler {
    state: Arc<AppState>,
}

impl Handler {
    fn new(api_keys: Vec<String>) -> Self {
        Self {
            state: Arc::new(AppState {
                api_manager: Mutex::new(ApiKeyManager::new(api_keys)),
                http_client: HttpClient::new(),
            }),
        }
    }

    async fn call_gemini_api(&self, prompt: &str) -> Result<String, String> {
        let max_retries = {
            let guard = self.state.api_manager.lock().await;
            guard.len()
        };
        if max_retries == 0 {
            return Err("Gemini API key が1つも設定されていません。".to_string());
        }
        for attempt in 0..max_retries {
            let api_key = {
                let guard = self.state.api_manager.lock().await;
                match guard.get_current_key() {
                    Some(key) if !key.trim().is_empty() => key,
                    _ => {
                        return Err("Gemini API key が空です。".to_string());
                    }
                }
            };

            // 利用可能なモデル名（必要に応じて変更してください）
            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-flash:generateContent?key={}",
                api_key
            );

            let system_prompt = r#"
あなたは優秀なプログラミングアシスタントです。
ユーザーの要望を理解し、実際に動作するプログラムを作成してください。
複数ファイルが必要な場合は、以下の形式でファイルごとに整理してください。

=== FILE: Cargo.toml ===
コード

=== FILE: src/main.rs ===
コード

重要:
* コードは省略しないでください。
* 必要なファイルはすべて出してください。
* ファイル名を明確にしてください。
"#;

            let full_prompt = format!("{}\n\nユーザーの要望:\n{}", system_prompt, prompt);
            let body = json!({
                "contents": [
                    {
                        "parts": [
                            {
                                "text": full_prompt
                            }
                        ]
                    }
                ]
            });

            let response = self
                .state
                .http_client
                .post(&url)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await;

            match response {
                Ok(response) => {
                    let status = response.status();
                    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        let mut guard = self.state.api_manager.lock().await;
                        guard.rotate();
                        continue;
                    }

                    if !status.is_success() {
                        let error_text = response.text().await.unwrap_or_else(|_| {
                            "Unknown API error".to_string()
                        });
                        return Err(format!("API Error (Status {}): {}", status, error_text));
                    }

                    let json_response: serde_json::Value = response.json().await.map_err(|e| {
                        format!("JSON Parse Error: {}", e)
                    })?;

                    let content = json_response
                        .get("candidates")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("content"))
                        .and_then(|c| c.get("parts"))
                        .and_then(|p| p.get(0))
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_str());

                    match content {
                        Some(text) if !text.trim().is_empty() => {
                            return Ok(text.to_string());
                        }
                        _ => {
                            return Err(format!(
                                "Invalid Gemini API response format: {}",
                                json_response
                            ));
                        }
                    }
                }
                Err(error) => {
                    return Err(format!("Network Request Error: {}", error));
                }
            }
        }
        Err("All Gemini API keys have reached their rate limits.".to_string())
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("Discord Bot is online! Logged in as: {}", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let current_user = match ctx.http.get_current_user().await {
            Ok(user) => user,
            Err(_) => return,
        };

        if !msg.mentions_user(&current_user) {
            return;
        }

        let prompt = msg
            .content
            .replace(&format!("<@!{}>", current_user.id), "")
            .replace(&format!("<@{}>", current_user.id), "")
            .trim()
            .to_string();

        if prompt.is_empty() {
            let _ = msg.channel_id.say(&ctx.http, "yo, where's the prompt? lol").await;
            return;
        }

        let _ = msg.channel_id.say(&ctx.http, "on it... cooking up the project rn").await;

        match self.call_gemini_api(&prompt).await {
            Ok(ai_response) => {
                let mut files = HashMap::new();
                files.insert("generated_code.txt".to_string(), ai_response);

                let mut zip_data = Vec::new();
                {
                    let cursor = std::io::Cursor::new(&mut zip_data);
                    let mut zip_writer = zip::ZipWriter::new(cursor);
                    let options = zip::write::FileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated);

                    for (filename, content) in &files {
                        if let Err(error) = zip_writer.start_file(filename, options) {
                            let _ = msg.channel_id.say(&ctx.http, format!("damn zip error: {}", error)).await;
                            return;
                        }
                        if let Err(error) = zip_writer.write_all(content.as_bytes()) {
                            let _ = msg.channel_id.say(&ctx.http, format!("rip write failed: {}", error)).await;
                            return;
                        }
                    }
                    if let Err(error) = zip_writer.finish() {
                        let _ = msg.channel_id.say(&ctx.http, format!("zip finish broke: {}", error)).await;
                        return;
                    }
                }

                let attachment = CreateAttachment::bytes(zip_data, "project.zip");
                let message = CreateMessage::new()
                    .content("here u go g, lmk if it works")
                    .add_file(attachment);

                let _ = msg.channel_id.send_message(&ctx.http, message).await;
            }
            Err(error) => {
                let _ = msg.channel_id.say(&ctx.http, format!("nah an error happened:\n{}", error)).await;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let mut api_keys = Vec::new();
    for variable_name in ["GEMINI_API_KEY_1", "GEMINI_API_KEY_2", "GEMINI_API_KEY_3"] {
        if let Ok(value) = std::env::var(variable_name) {
            if !value.trim().is_empty() {
                api_keys.push(value);
            }
        }
    }

    if api_keys.is_empty() {
        eprintln!("ERROR: No Gemini API keys are configured.");
        return;
    }

    let token = match std::env::var("DISCORD_BOT_TOKEN") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("ERROR: DISCORD_BOT_TOKEN is not set.");
            return;
        }
    };

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let handler = Handler::new(api_keys);
    let mut client = Client::builder(&token, intents)
        .event_handler(handler)
        .await
        .expect("Failed to create client");

    if let Err(error) = client.start().await {
        eprintln!("Discord client error: {:?}", error);
    }
}
