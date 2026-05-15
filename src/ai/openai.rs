use std::{
    collections::BTreeMap,
    io::Write,
    process::{Command, Stdio},
};

use serde_json::{Number, Value};

use super::{
    content_blocks_to_text, json_escape, AiContext, AiError, AiMessage, ApiProvider,
    AssistantOutput, ContentBlock, Model, ModelCapabilities, StopReason,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiConfig {
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    pub capabilities: ModelCapabilities,
}

#[derive(Debug)]
pub struct OpenAiCompatibleProvider {
    config: OpenAiConfig,
    model: Model,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: OpenAiConfig) -> Self {
        let config = OpenAiConfig {
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            ..config
        };
        let model = Model::new(
            config.model_id.clone(),
            "openai",
            "openai-chat-completions",
            config.capabilities,
        );

        Self { config, model }
    }
}

impl ApiProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn model(&self) -> &Model {
        &self.model
    }

    fn complete(&self, model: &Model, context: &AiContext) -> Result<AssistantOutput, AiError> {
        let request_body = chat_request_body(&model.id, context);
        let response = run_curl(
            &format!("{}/chat/completions", self.config.base_url),
            &self.config.api_key,
            &request_body,
        )?;

        if response.status_code >= 400 {
            return Err(AiError::ProviderFailed(format!(
                "provider returned HTTP {}: {}",
                response.status_code,
                provider_error_message(&response.body)
            )));
        }

        parse_chat_response(&response.body)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpResponse {
    status_code: u16,
    body: String,
}

fn run_curl(url: &str, api_key: &str, body: &str) -> Result<HttpResponse, AiError> {
    let mut child = Command::new("curl")
        .arg("-sS")
        .arg("--connect-timeout")
        .arg("20")
        .arg("--max-time")
        .arg("90")
        .arg("-w")
        .arg("\n__PICOCODE_HTTP_STATUS__:%{http_code}")
        .arg("-X")
        .arg("POST")
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("-H")
        .arg(format!("Authorization: Bearer {api_key}"))
        .arg("--data-binary")
        .arg("@-")
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(AiError::Io)?;

    child
        .stdin
        .as_mut()
        .expect("curl stdin should be piped")
        .write_all(body.as_bytes())
        .map_err(AiError::Io)?;

    let output = child.wait_with_output().map_err(AiError::Io)?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(AiError::ProviderFailed(if stderr.is_empty() {
            "curl request failed without stderr".to_owned()
        } else {
            stderr
        }));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let Some((body, status_code)) = split_curl_status(&stdout) else {
        return Err(AiError::ProviderFailed(format!(
            "curl response did not include HTTP status: {}",
            response_preview(&stdout)
        )));
    };

    Ok(HttpResponse { status_code, body })
}

fn chat_request_body(model_id: &str, context: &AiContext) -> String {
    let mut messages = Vec::new();
    if let Some(system_prompt) = &context.system_prompt {
        messages.push(format!(
            "{{\"role\":\"system\",\"content\":\"{}\"}}",
            json_escape(system_prompt)
        ));
    }

    messages.extend(context.messages.iter().filter_map(chat_message_json));

    let tools = tools_json(context);
    format!(
        "{{\"model\":\"{}\",\"messages\":[{}]{} }}",
        json_escape(model_id),
        messages.join(","),
        tools
    )
}

fn chat_message_json(message: &AiMessage) -> Option<String> {
    match message {
        AiMessage::User(message) => Some(user_message_json(message)),
        AiMessage::Assistant(message) => Some(format!(
            "{{\"role\":\"assistant\",\"content\":\"{}\"{}}}",
            json_escape(&content_blocks_to_text(&message.content)),
            assistant_tool_calls_json(message)
        )),
        AiMessage::ToolResult(message) => Some(format!(
            "{{\"role\":\"tool\",\"tool_call_id\":\"{}\",\"content\":\"{}\"}}",
            json_escape(&message.tool_call_id),
            json_escape(&content_blocks_to_text(&message.content))
        )),
    }
}

fn user_message_json(message: &super::UserMessage) -> String {
    let has_images = message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::Image(_)));
    if !has_images {
        return format!(
            "{{\"role\":\"user\",\"content\":\"{}\"}}",
            json_escape(&content_blocks_to_text(&message.content))
        );
    }

    let parts = message
        .content
        .iter()
        .filter_map(user_content_part_json)
        .collect::<Vec<_>>();
    format!("{{\"role\":\"user\",\"content\":[{}]}}", parts.join(","))
}

fn user_content_part_json(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text(text) => Some(format!(
            "{{\"type\":\"text\",\"text\":\"{}\"}}",
            json_escape(&text.text)
        )),
        ContentBlock::Image(image) => Some(format!(
            "{{\"type\":\"image_url\",\"image_url\":{{\"url\":\"{}\",\"detail\":\"auto\"}}}}",
            json_escape(&image.data_url)
        )),
        ContentBlock::Thinking(_) | ContentBlock::ToolCall(_) => None,
    }
}

fn assistant_tool_calls_json(message: &super::AssistantMessage) -> String {
    let tool_calls = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(call),
            ContentBlock::Text(_) | ContentBlock::Thinking(_) | ContentBlock::Image(_) => None,
        })
        .map(|call| {
            let arguments = tool_arguments_json_string(&call.arguments);
            format!(
                "{{\"id\":\"{}\",\"type\":\"function\",\"function\":{{\"name\":\"{}\",\"arguments\":\"{}\"}}}}",
                json_escape(&call.id),
                json_escape(&call.name),
                json_escape(&arguments)
            )
        })
        .collect::<Vec<_>>();

    if tool_calls.is_empty() {
        String::new()
    } else {
        format!(",\"tool_calls\":[{}]", tool_calls.join(","))
    }
}

fn tool_arguments_json_string(arguments: &str) -> String {
    let trimmed = arguments.trim();
    if trimmed.starts_with('{') && serde_json::from_str::<Value>(trimmed).is_ok() {
        return trimmed.to_owned();
    }

    let values = arguments
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), tool_argument_value(value.trim())))
        .collect::<BTreeMap<_, _>>();

    serde_json::to_string(&values).unwrap_or_else(|_| "{}".to_owned())
}

fn tool_argument_value(value: &str) -> Value {
    if value == "true" {
        return Value::Bool(true);
    }
    if value == "false" {
        return Value::Bool(false);
    }
    if let Ok(number) = value.parse::<u64>() {
        return Value::Number(Number::from(number));
    }
    Value::String(value.to_owned())
}

fn tools_json(context: &AiContext) -> String {
    if context.tools.is_empty() {
        return String::new();
    }

    let tools = context
        .tools
        .iter()
        .map(|tool| {
            format!(
                "{{\"type\":\"function\",\"function\":{{\"name\":\"{}\",\"description\":\"{}\",\"parameters\":{}}}}}",
                json_escape(&tool.name),
                json_escape(&tool.description),
                tool.input_schema_json
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    format!(",\"tools\":[{tools}],\"tool_choice\":\"auto\"")
}

fn parse_chat_response(response: &str) -> Result<AssistantOutput, AiError> {
    let value = serde_json::from_str::<Value>(response).map_err(|error| {
        AiError::ProviderFailed(format!(
            "provider returned invalid JSON ({error}): {}",
            response_preview(response)
        ))
    })?;
    if value.get("error").is_some() {
        return Err(AiError::ProviderFailed(provider_error_from_value(&value)));
    }

    let Some(message) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
    else {
        return Err(AiError::ProviderFailed(format!(
            "provider response did not include choices[0].message: {}",
            response_preview(response)
        )));
    };

    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let tool_calls = parse_tool_calls(message);
    if tool_calls.is_empty() {
        return Ok(AssistantOutput::from_provider_content(content));
    }

    let mut blocks = Vec::new();
    if !content.trim().is_empty() {
        blocks.append(
            &mut AssistantOutput::from_provider_content(content)
                .message
                .content,
        );
    }
    blocks.extend(
        tool_calls
            .into_iter()
            .map(|call| ContentBlock::tool_call(call.id, call.name, call.arguments)),
    );

    Ok(AssistantOutput {
        message: super::AssistantMessage {
            content: blocks,
            stop_reason: Some(StopReason::ToolUse),
            usage: None,
        },
        raw_response_id: None,
    })
}

fn split_curl_status(stdout: &str) -> Option<(String, u16)> {
    let (body, status) = stdout.rsplit_once("\n__PICOCODE_HTTP_STATUS__:")?;
    Some((body.to_owned(), status.trim().parse::<u16>().ok()?))
}

fn provider_error_message(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(value) => provider_error_from_value(&value),
        Err(_) => response_preview(body),
    }
}

fn provider_error_from_value(value: &Value) -> String {
    let Some(error) = value.get("error") else {
        return response_preview(&value.to_string());
    };

    if let Some(message) = error.as_str() {
        return message.to_owned();
    }

    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown provider error");
    let error_type = error.get("type").and_then(Value::as_str);
    let code = error.get("code").and_then(|code| {
        code.as_str()
            .map(str::to_owned)
            .or_else(|| Some(code.to_string()))
    });

    match (error_type, code) {
        (Some(error_type), Some(code)) => format!("{message} (type={error_type}, code={code})"),
        (Some(error_type), None) => format!("{message} (type={error_type})"),
        (None, Some(code)) => format!("{message} (code={code})"),
        (None, None) => message.to_owned(),
    }
}

fn response_preview(response: &str) -> String {
    const LIMIT: usize = 1200;
    let trimmed = response.trim();
    let preview = trimmed.chars().take(LIMIT).collect::<String>();
    if trimmed.chars().count() > LIMIT {
        format!("{preview}...")
    } else if preview.is_empty() {
        "<empty response body>".to_owned()
    } else {
        preview
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedToolCall {
    id: String,
    name: String,
    arguments: String,
}

fn parse_tool_calls(message: &Value) -> Vec<ParsedToolCall> {
    let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return Vec::new();
    };
    tool_calls
        .iter()
        .enumerate()
        .filter_map(|(index, call)| {
            let function = call.get("function")?;
            Some(ParsedToolCall {
                id: call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("call-{index}")),
                name: function.get("name")?.as_str()?.to_owned(),
                arguments: function.get("arguments")?.as_str()?.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{AiContext, AiMessage, ToolSpec};

    #[test]
    fn request_body_serializes_context() {
        let context = AiContext::new(
            Some("rules".to_owned()),
            vec![
                AiMessage::user_text("hello"),
                AiMessage::assistant_text("hi"),
            ],
        );

        let body = chat_request_body("model", &context);

        assert!(body.contains("\"model\":\"model\""));
        assert!(body.contains("\"role\":\"system\",\"content\":\"rules\""));
        assert!(body.contains("\"role\":\"user\",\"content\":\"hello\""));
        assert!(body.contains("\"role\":\"assistant\",\"content\":\"hi\""));
    }

    #[test]
    fn request_body_serializes_image_messages() {
        let context = AiContext::new(
            None,
            vec![AiMessage::user_content(vec![
                ContentBlock::image(
                    "./shot.png",
                    "shot.png",
                    "image/png",
                    "data:image/png;base64,AAAA",
                ),
                ContentBlock::text("what do you see?"),
            ])],
        );

        let body = chat_request_body("model", &context);

        assert!(body.contains("\"role\":\"user\",\"content\":["));
        assert!(body.contains("\"type\":\"image_url\""));
        assert!(body.contains("\"data:image/png;base64,AAAA\""));
        assert!(body.contains("\"type\":\"text\",\"text\":\"what do you see?\""));
    }

    #[test]
    fn request_body_serializes_tools() {
        let mut context = AiContext::new(None, vec![AiMessage::user_text("inspect")]);
        context.tools = vec![ToolSpec {
            name: "read".to_owned(),
            description: "Read a file".to_owned(),
            input_schema_json: "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}},\"required\":[\"path\"]}".to_owned(),
        }];

        let body = chat_request_body("model", &context);

        assert!(body.contains("\"tools\""));
        assert!(body.contains("\"name\":\"read\""));
        assert!(body.contains("\"tool_choice\":\"auto\""));
    }

    #[test]
    fn request_body_serializes_tool_result_messages() {
        let context = AiContext::new(
            None,
            vec![
                AiMessage::Assistant(super::super::AssistantMessage {
                    content: vec![ContentBlock::tool_call("call-1", "read", "path=Cargo.toml")],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: None,
                }),
                AiMessage::tool_result("call-1", "path: Cargo.toml", false),
            ],
        );

        let body = chat_request_body("model", &context);

        assert!(body.contains("\"tool_calls\""));
        assert!(body.contains("\"role\":\"tool\""));
        assert!(body.contains("\"tool_call_id\":\"call-1\""));
        assert!(body.contains("\"content\":\"path: Cargo.toml\""));
        assert!(body.contains("\"arguments\":\"{\\\"path\\\":\\\"Cargo.toml\\\"}\""));
    }

    #[test]
    fn tool_arguments_json_string_converts_key_value_lines() {
        let arguments =
            tool_arguments_json_string("query=ToolRuntime\npath=src\nlimit=20\nignore_case=true");

        assert_eq!(
            arguments,
            "{\"ignore_case\":true,\"limit\":20,\"path\":\"src\",\"query\":\"ToolRuntime\"}"
        );
    }

    #[test]
    fn tool_arguments_json_string_preserves_json_arguments() {
        let arguments = tool_arguments_json_string("{\"path\":\"Cargo.toml\",\"limit\":20}");

        assert_eq!(arguments, "{\"path\":\"Cargo.toml\",\"limit\":20}");
    }

    #[test]
    fn parse_chat_response_extracts_tool_calls() {
        let response = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"Cargo.toml\",\"offset\":0,\"limit\":20}"}}]}}]}"#;

        let output = parse_chat_response(response).unwrap();
        let calls = output.tool_calls();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "read");
        assert_eq!(
            calls[0].arguments,
            "{\"path\":\"Cargo.toml\",\"offset\":0,\"limit\":20}"
        );
    }

    #[test]
    fn parse_chat_response_surfaces_provider_error() {
        let response = r#"{"error":{"message":"tools are not supported by this model","type":"invalid_request_error","code":"unsupported_tools"}}"#;

        let error = parse_chat_response(response).unwrap_err().to_string();

        assert!(error.contains("tools are not supported by this model"));
        assert!(error.contains("invalid_request_error"));
        assert!(error.contains("unsupported_tools"));
    }

    #[test]
    fn parse_chat_response_surfaces_unexpected_body() {
        let response = r#"{"id":"resp_1","object":"chat.completion"}"#;

        let error = parse_chat_response(response).unwrap_err().to_string();

        assert!(error.contains("choices[0].message"));
        assert!(error.contains("resp_1"));
    }

    #[test]
    fn split_curl_status_extracts_body_and_status() {
        let stdout = "{\"ok\":false}\n__PICOCODE_HTTP_STATUS__:400";

        let (body, status) = split_curl_status(stdout).unwrap();

        assert_eq!(body, "{\"ok\":false}");
        assert_eq!(status, 400);
    }

    #[test]
    fn provider_error_message_falls_back_to_body_preview() {
        let message = provider_error_message("upstream unavailable");

        assert_eq!(message, "upstream unavailable");
    }

    #[test]
    fn config_trims_base_url_slash() {
        let config = OpenAiConfig {
            base_url: "https://example.com/v1/".to_owned(),
            api_key: "key".to_owned(),
            model_id: "model".to_owned(),
            capabilities: ModelCapabilities::default(),
        };

        let provider = OpenAiCompatibleProvider::new(config);

        assert_eq!(provider.config.base_url, "https://example.com/v1");
        assert_eq!(provider.model.id, "model");
    }
}
