use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::Mutex;
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;
use serenity::builder::CreateAttachment;
use reqwest::Client as HttpClient;
use serde_json::json;

struct ApiKeyManager {
    keys: Vec<String>,
    current_index: usize,
}

impl ApiKeyManager {
    fn new(keys: Vec<String>) -> Self {
        Self { keys, current_index: 0 }
    }

    fn get_current_key(&self) -> String {
        self.keys[self.current_index].clone()
    }

    fn rotate(&mut self) {
        if self.keys.is_empty() { return; }
        self.current_index = (self.current_index + 1) % self.keys.len();
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
            guard.keys.len()
        };

        for _ in 0..max_retries {
            let api_key = {
                let guard = self.state.api_manager.lock().await;
                guard.get_current_key()
            };

            let body = json!({
                "contents": [
                    {
                        "parts": [
                            {
                                "text": format!("あなたは優秀なプログラミングアシスタントです。ユーザーの要望に基づき、複数のファイル構成とコードを考えてください。出力は、各ファイル名とコードをわかりやすく整理して返すか、プログラムがパースしやすい形式にしてください。\n\n要望: {}", prompt)
                            }
                        ]
                    }
                ]
            });

            // ここを現在の最新モデル "gemini-2.5-flash" に変更
            let url = format!("https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key={}", api_key);

            let res = self.state.http_client
                .post(&url)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await;

            match res {
                Ok(response) => {
                    let status = response.status();
                    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.as_u16() == 429 {
                        let mut guard = self.state.api_manager.lock().await;
                        guard.rotate();
                        continue;
                    }

                    if !status.is_success() {
                        let err_text = response.text().await.unwrap_or_default();
                        return Err(format!("API Error (Status {}): {}", status, err_text));
                    }

                    let json_res: serde_json::Value = response.json().await
                        .map_err(|e| format!("JSON Parse Error: {}", e))?;
                    
                    let content = json_res["candidates"][0]["content"]["parts"][0]["text"]
                        .as_str()
                        .ok_or("Invalid API response format")?
                        .to_string();

                    return Ok(content);
                }
                Err(e) => {
                    return Err(format!("Network Request Error: {}", e));
                }
            }
        }

        Err("All Gemini API keys have reached their rate limits.".to_string())
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("{} is online.", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let current_user = match ctx.http.get_current_user().await {
            Ok(user) => user,
            Err(_) => return,
        };

        let is_mentioned = msg.mentions_user(&current_user);
        if !is_mentioned {
            return;
        }

        let prompt = msg.content
            .replace(&format!("<@!{}>", current_user.id), "")
            .replace(&format!("<@{}>", current_user.id), "")
            .trim()
            .to_string();

        if prompt.is_empty() {
            let _ = msg.channel_id.say(&ctx.http, "yo, where's the prompt? lol").await;
            return;
        }

        let _ = msg.channel_id.say(&ctx.http, "on it... cooking up the zip for u rn").await;

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
                        if let Err(e) = zip_writer.start_file(filename, options) {
                            let _ = msg.channel_id.say(&ctx.http, format!("damn zip error: {}", e)).await;
                            return;
                        }
                        if let Err(e) = zip_writer.write_all(content.as_bytes()) {
                            let _ = msg.channel_id.say(&ctx.http, format!("rip write failed: {}", e)).await;
                            return;
                        }
                    }
                    if let Err(e) = zip_writer.finish() {
                        let _ = msg.channel_id.say(&ctx.http, format!("zip finish broke: {}", e)).await;
                        return;
                    }
                }

                let attachment = CreateAttachment::bytes(zip_data, "project.zip");
                let _ = msg.channel_id.send_files(&ctx.http, vec![attachment], serenity::builder::CreateMessage::new().content("here u go g, lmk if it works")).await;
            }
            Err(e) => {
                let _ = msg.channel_id.say(&ctx.http, format!("nah an error happened: {}", e)).await;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let api_keys = vec![
        std::env::var("GEMINI_API_KEY_1").expect("GEMINI_API_KEY_1 is not set"),
        std::env::var("GEMINI_API_KEY_2").expect("GEMINI_API_KEY_2 is not set"),
        std::env::var("GEMINI_API_KEY_3").expect("GEMINI_API_KEY_3 is not set"),
    ];

    let token = std::env::var("DISCORD_BOT_TOKEN").expect("DISCORD_BOT_TOKEN is not set");
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let handler = Handler::new(api_keys);
    let mut client = Client::builder(&token, intents)
        .event_handler(handler)
        .await
        .expect("Failed to create client");

    if let Err(why) = client.start().await {
        println!("Client error: {:?}", why);
    }
}
