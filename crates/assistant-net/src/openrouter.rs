//! OpenRouter's chat completions and model list, which reach many
//! companies' models through one API.

use std::sync::atomic::AtomicBool;

use assistant_keys::ApiKey;
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::http::{Client, Host, Method, NetError, Request, Response};
use assistant_core::Picodollars;
use assistant_core::cost::Price;

use crate::provider::{Citation, Part, Schema, is_jpeg, parse, status_error};

/// Pages of the model list read at most.
const MAX_MODEL_PAGES: usize = 10;
const MODELS_PER_PAGE: usize = 1000;

/// Web search run by OpenRouter during the request: the provider's own
/// for models that have one, such as Claude's, and Exa's otherwise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebSearch {
    /// Results for each search.
    pub max_results: u32,
    /// The most searches one request may make.
    pub max_uses: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: String,
    pub user: Vec<Part>,
    pub web_search: Option<WebSearch>,
    /// The JSON Schema the answer must follow. Not with web search, as
    /// with Anthropic.
    pub schema: Option<Schema>,
}

/// Why the model stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finish {
    Stop,
    /// Cut off: the answer is incomplete and is not used.
    Length,
    /// The model or its provider declined; the answer is not used.
    ContentFilter,
    ToolCalls,
    Other(String),
}

/// What a request used and cost, as OpenRouter reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub web_searches: u64,
    /// What OpenRouter charged, when it says.
    pub cost: Option<Picodollars>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub id: String,
    pub model: String,
    pub text: String,
    pub finish: Finish,
    /// The pages the answer cites, each once, in order.
    pub citations: Vec<Citation>,
    pub usage: Usage,
}

/// A model the owner can pick, with what it can do and its prices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub image_input: bool,
    pub structured_outputs: bool,
    pub tools: bool,
    /// Per token, read as sent.
    pub prompt_price: Option<Picodollars>,
    pub completion_price: Option<Picodollars>,
    /// Per input image.
    pub image_price: Option<Picodollars>,
    /// Per search.
    pub web_search_price: Option<Picodollars>,
}

impl ModelInfo {
    /// The model's prices, if OpenRouter lists its token prices. Images
    /// and searches it lists no price for count as free, which only an
    /// unusual model would be.
    pub fn price(&self) -> Option<Price> {
        Some(Price {
            input: self.prompt_price?,
            output: self.completion_price?,
            cache_read: self.prompt_price?,
            cache_write: self.prompt_price?,
            search: self.web_search_price.unwrap_or_default(),
            image: self.image_price.unwrap_or_default(),
            long_prompt: None,
        })
    }
}

impl Usage {
    /// What the request used, for the price table; the images it sent
    /// are the caller's to add, as OpenRouter does not count them.
    pub fn counted(&self) -> assistant_core::cost::Usage {
        assistant_core::cost::Usage {
            input_tokens: self.prompt_tokens,
            output_tokens: self.completion_tokens,
            searches: self.web_searches,
            ..assistant_core::cost::Usage::default()
        }
    }
}

/// A model id as OpenRouter writes them, such as
/// `anthropic/claude-opus-5.5`, safe to show and to send back.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'/' | b':'))
}

fn price(model: &Value, field: &str) -> Option<Picodollars> {
    Picodollars::parse(model["pricing"][field].as_str()?)
}

fn model_info(model: &Value) -> Option<ModelInfo> {
    let id = model["id"].as_str().filter(|id| plain_id(id))?;
    let has = |list: &Value, item: &str| {
        list.as_array()
            .is_some_and(|l| l.iter().any(|v| v.as_str() == Some(item)))
    };
    Some(ModelInfo {
        id: id.to_owned(),
        name: model["name"].as_str().unwrap_or(id).to_owned(),
        image_input: has(&model["architecture"]["input_modalities"], "image"),
        structured_outputs: has(&model["supported_parameters"], "structured_outputs"),
        tools: has(&model["supported_parameters"], "tools"),
        prompt_price: price(model, "prompt"),
        completion_price: price(model, "completion"),
        image_price: price(model, "image"),
        web_search_price: price(model, "web_search"),
    })
}

/// The JSON body of a chat completion request.
pub(crate) fn request_body(request: &ChatRequest) -> Result<Value, NetError> {
    if request.user.is_empty() {
        return Err(NetError::Invalid(
            "a request needs the owner's photos or text",
        ));
    }
    if request.schema.is_some() && request.web_search.is_some() {
        return Err(NetError::Invalid(
            "a structured answer cannot also search the web",
        ));
    }
    let mut content = Vec::new();
    for part in &request.user {
        content.push(match part {
            Part::Text(text) => json!({"type": "text", "text": text}),
            Part::Jpeg(bytes) => {
                if !is_jpeg(bytes) {
                    return Err(NetError::Invalid("a photo is not a JPEG"));
                }
                json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:image/jpeg;base64,{}", crate::base64::encode(bytes))}
                })
            }
        });
    }
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({"role": "system", "content": request.system}));
    }
    messages.push(json!({"role": "user", "content": content}));
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "messages": messages,
        // Route only to providers that support every parameter sent, so a
        // schema is never silently ignored, and that do not keep or train
        // on the owner's photos and text.
        "provider": {"require_parameters": true, "data_collection": "deny"},
    });
    if let Some(search) = &request.web_search {
        body["tools"] = json!([{
            "type": "openrouter:web_search",
            "parameters": {"max_results": search.max_results, "max_uses": search.max_uses},
        }]);
    }
    if let Some(schema) = &request.schema {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {"name": schema.name, "strict": true, "schema": schema.schema},
        });
    }
    Ok(body)
}

#[derive(Deserialize)]
struct RawAnswer {
    #[serde(default)]
    id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    choices: Vec<Value>,
    #[serde(default)]
    usage: Value,
    #[serde(default)]
    error: Option<Value>,
}

/// Reads a 200 answer, which may still hold an error that happened once
/// the model had started.
fn answer(response: &Response, key: &str) -> Result<Answer, NetError> {
    let raw: RawAnswer = parse(response)?;
    if raw.error.is_some() {
        return Err(status_error(response, key));
    }
    let choice = raw
        .choices
        .first()
        .ok_or_else(|| NetError::BadResponse("the answer has no choices".into()))?;
    if choice.get("error").is_some_and(|e| !e.is_null()) {
        let wrapped = Response {
            status: response.status,
            retry_after: None,
            body: serde_json::to_vec(&json!({"error": choice["error"]})).unwrap_or_default(),
        };
        return Err(status_error(&wrapped, key));
    }
    let message = &choice["message"];
    let finish = match choice["finish_reason"].as_str() {
        Some("stop") => Finish::Stop,
        Some("length") => Finish::Length,
        Some("content_filter") => Finish::ContentFilter,
        Some("tool_calls") => Finish::ToolCalls,
        other => Finish::Other(other.unwrap_or("").to_owned()),
    };
    let mut citations = Vec::<Citation>::new();
    for note in message["annotations"].as_array().into_iter().flatten() {
        if note["type"] != "url_citation" {
            continue;
        }
        let cited = &note["url_citation"];
        let Some(url) = cited["url"].as_str() else {
            continue;
        };
        if citations.iter().any(|c| c.url == url) {
            continue;
        }
        citations.push(Citation {
            url: url.to_owned(),
            title: cited["title"].as_str().unwrap_or("").to_owned(),
            cited_text: cited["content"].as_str().unwrap_or("").to_owned(),
        });
    }
    let count = |v: &Value| v.as_u64().unwrap_or(0);
    Ok(Answer {
        id: raw.id,
        model: raw.model,
        text: message["content"].as_str().unwrap_or("").to_owned(),
        finish,
        citations,
        usage: Usage {
            prompt_tokens: count(&raw.usage["prompt_tokens"]),
            completion_tokens: count(&raw.usage["completion_tokens"]),
            web_searches: count(&raw.usage["server_tool_use"]["web_search_requests"]),
            // A JSON number written as a decimal reads back exactly.
            cost: raw.usage["cost"]
                .as_number()
                .and_then(|n| Picodollars::parse(&n.to_string())),
        },
    })
}

/// OpenRouter's API, called with the owner's key.
pub struct OpenRouter<'a> {
    client: &'a Client,
    key: &'a ApiKey,
}

impl<'a> OpenRouter<'a> {
    pub fn new(client: &'a Client, key: &'a ApiKey) -> Self {
        Self { client, key }
    }

    fn headers(&self, json: bool) -> Vec<(&'static str, Zeroizing<String>)> {
        let mut headers = vec![(
            "authorization",
            Zeroizing::new(format!("Bearer {}", self.key.expose())),
        )];
        if json {
            headers.push((
                "content-type",
                Zeroizing::new("application/json".to_owned()),
            ));
        }
        headers
    }

    /// The models that take photos and keep to a schema, which the app
    /// needs.
    pub fn models(&self, cancel: &AtomicBool) -> Result<Vec<ModelInfo>, NetError> {
        let mut models = Vec::new();
        for page in 0..MAX_MODEL_PAGES {
            let path = format!(
                "/api/v1/models?input_modalities=image&supported_parameters=structured_outputs&limit={MODELS_PER_PAGE}&offset={}",
                page * MODELS_PER_PAGE
            );
            let response = crate::retry::send(
                self.client,
                || Request {
                    host: Host::OpenRouter,
                    method: Method::Get,
                    path: path.clone(),
                    headers: self.headers(false),
                    body: None,
                },
                cancel,
            )?;
            if response.status != 200 {
                return Err(status_error(&response, self.key.expose()));
            }
            let body: Value = parse(&response)?;
            let data = body["data"]
                .as_array()
                .ok_or_else(|| NetError::BadResponse("the model list has no data".into()))?;
            models.extend(data.iter().filter_map(model_info));
            if data.len() < MODELS_PER_PAGE || body["links"]["next"].is_null() {
                break;
            }
        }
        Ok(models)
    }

    pub fn chat(&self, request: &ChatRequest, cancel: &AtomicBool) -> Result<Answer, NetError> {
        let body = serde_json::to_vec(&request_body(request)?)
            .map_err(|_| NetError::Invalid("the request could not be written"))?;
        let response = crate::retry::send(
            self.client,
            || Request {
                host: Host::OpenRouter,
                method: Method::Post,
                path: "/api/v1/chat/completions".into(),
                headers: self.headers(true),
                body: Some(body.clone()),
            },
            cancel,
        )?;
        if response.status != 200 {
            return Err(status_error(&response, self.key.expose()));
        }
        answer(&response, self.key.expose())
    }
}

#[cfg(test)]
mod tests;
