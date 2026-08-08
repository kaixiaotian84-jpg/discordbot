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

    async fn call_deepseek_api(&self, prompt: &str) -> Result<String, String> {
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
                "model": "deepseek-chat",
                "messages": [
                    {
                        "role": "system",
                        "content": "あなたは優秀なプログラミングアシスタントです。ユーザーの要望に基づき、複数のファイル構成とコードを考えてください。出力は、各ファイル名とコードをわかりやすく整理して返すか、プログラムがパースしやすい形式にしてください。コード内には//コメント等を含めずにかつ、確実に.zipファイルで生成してください。"
                    },
                    {
                        "role": "user",
                        "content": prompt
                    }
                ],
                "stream": false
            });

            let res = self.state.http_client
                .post("https://api.deepseek.com/chat/completions")
                .header("Authorization", format!("Bearer {}", api_key))
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
                    
                    let content = json_res["choices"][0]["message"]["content"]
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

        Err("All DeepSeek API keys have reached their rate limits.".to_string())
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

        let is_mentioned = msg.mentions_user(&ctx.cache.current_user());
        if !is_mentioned {
            return;
        }

        let prompt = msg.content
            .replace(&format!("<@!{}>", ctx.cache.current_user().id), "")
            .replace(&format!("<@{}>", ctx.cache.current_user().id), "")
            .trim()
            .to_string();

        if prompt.is_empty() {
            let _ = msg.channel_id.say(&ctx.http, "Please provide a prompt.").await;
            return;
        }

        let _ = msg.channel_id.say(&ctx.http, "Generating code and creating ZIP...").await;

        match self.call_deepseek_api(&prompt).await {
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
                            let _ = msg.channel_id.say(&ctx.http, format!("ZIP Error: {}", e)).await;
                            return;
                        }
                        if let Err(e) = zip_writer.write_all(content.as_bytes()) {
                            let _ = msg.channel_id.say(&ctx.http, format!("ZIP Write Error: {}", e)).await;
                            return;
                        }
                    }
                    if let Err(e) = zip_writer.finish() {
                        let _ = msg.channel_id.say(&ctx.http, format!("ZIP Finish Error: {}", e)).await;
                        return;
                    }
                }

                let attachment = CreateAttachment::bytes(zip_data, "project.zip");
                let _ = msg.channel_id.send_files(&ctx.http, vec![attachment], serenity::builder::CreateMessage::new().content("Complete!")).await;
            }
            Err(e) => {
                let _ = msg.channel_id.say(&ctx.http, format!("Error: {}", e)).await;
            }
        }
    }
}
#[tokio::main]
async fn main() {
    let api_keys = vec![
        "sk-99836edc963b4d15b3687c660abd6ba9".to_string(),
    ];

    let token = "MTUzNTY2NDQyNDY2NzU4NjY2MA.GDUCka.B0_jD1xs6qczJ6WFzyh5-Yfz-LNu-z1DunyQz8".to_string();
    
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
