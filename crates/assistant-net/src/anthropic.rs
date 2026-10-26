//! Anthropic's Messages and Models APIs, called directly over HTTPS, as
//! Anthropic has no Rust SDK.

use std::sync::atomic::AtomicBool;

use assistant_core::cost::Usage;
use assistant_keys::ApiKey;
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::http::{Client, Host, Method, NetError, Request};
use crate::provider::{Citation, Effort, Part, Schema, is_jpeg, parse, status_error};

/// The API version the requests are written for.
pub const API_VERSION: &str = "2026-10-21";
/// The model used unless the owner picks another.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// The server-side web search tool.
const WEB_SEARCH_TOOL: &str = "web_search_20260209";
/// The server-side tool that reads a whole page.
const WEB_FETCH_TOOL: &str = "web_fetch_20260209";
/// The most passages kept from one page; a search's are at most 150
/// characters each.
const MAX_PASSAGES: usize = 12;
/// The most characters kept of one passage from a page read whole, which
/// may cite a long stretch of it.
const MAX_PASSAGE_CHARS: usize = 400;
/// Pages of the model list read at most.
const MAX_MODEL_PAGES: usize = 10;

/// Web search run by Anthropic during the request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebSearch {
    /// The most searches one request may make.
    pub max_uses: u32,
    /// Only these sites, if any are named. Not with `blocked_domains`.
    pub allowed_domains: Vec<String>,
    /// Never these sites.
    pub blocked_domains: Vec<String>,
}

/// Whole pages read by Anthropic during the request. It can read only
/// pages whose address has already appeared in the request or in its
/// search results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebFetch {
    /// The most pages one request may read.
    pub max_uses: u32,
    /// Only these sites. Never empty: the app names the sites it reads.
    pub allowed_domains: Vec<String>,
    /// The most tokens kept of one page.
    pub max_content_tokens: u32,
}

/// One request: the app's fixed instructions, the owner's photos and
/// text, and what the answer may use and must look like.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: String,
    pub user: Vec<Part>,
    /// A paused answer's content, sent back unchanged to let it finish.
    pub resume: Vec<Value>,
    pub web_search: Option<WebSearch>,
    /// Only with web search, which finds the pages to read.
    pub web_fetch: Option<WebFetch>,
    /// The JSON Schema the answer must follow. Not with web search, whose
    /// answers always carry citations, which a schema-held answer cannot.
    pub schema: Option<Schema>,
    pub effort: Option<Effort>,
}

/// Why the model stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    EndTurn,
    /// Cut off: the answer is incomplete and is not used.
    MaxTokens,
    StopSequence,
    ToolUse,
    /// A long turn was paused; send its content back to let it finish.
    PauseTurn,
    /// The model declined; the answer is not used.
    Refusal {
        category: Option<String>,
    },
    Other(String),
}

/// The model's answer. Its content is kept as sent, so a paused answer
/// can be sent back exactly.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: String,
    pub model: String,
    pub stop: Stop,
    pub content: Vec<Value>,
    /// What it used, which Anthropic's price table turns into its cost.
    pub usage: Usage,
}

#[derive(Deserialize)]
struct RawMessage {
    id: String,
    model: String,
    stop_reason: Option<String>,
    #[serde(default)]
    stop_details: Option<Value>,
    content: Vec<Value>,
    usage: RawUsage,
}

#[derive(Deserialize)]
struct RawUsage {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    server_tool_use: Option<RawServerToolUse>,
}

#[derive(Deserialize)]
struct RawServerToolUse {
    #[serde(default)]
    web_search_requests: u64,
}

impl Message {
    fn from_raw(raw: RawMessage) -> Self {
        let stop = match raw.stop_reason.as_deref() {
            Some("end_turn") => Stop::EndTurn,
            Some("max_tokens") => Stop::MaxTokens,
            Some("stop_sequence") => Stop::StopSequence,
            Some("tool_use") => Stop::ToolUse,
            Some("pause_turn") => Stop::PauseTurn,
            Some("refusal") => Stop::Refusal {
                category: raw
                    .stop_details
                    .as_ref()
                    .and_then(|d| d["category"].as_str())
                    .map(str::to_owned),
            },
            other => Stop::Other(other.unwrap_or("").to_owned()),
        };
        Self {
            id: raw.id,
            model: raw.model,
            stop,
            content: raw.content,
            usage: Usage {
                input_tokens: raw.usage.input_tokens,
                output_tokens: raw.usage.output_tokens,
                cache_write_tokens: raw.usage.cache_creation_input_tokens.unwrap_or(0),
                cache_read_tokens: raw.usage.cache_read_input_tokens.unwrap_or(0),
                searches: raw
                    .usage
                    .server_tool_use
                    .map_or(0, |s| s.web_search_requests),
                images: 0,
            },
        }
    }

    fn blocks<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
        self.content.iter().filter(move |b| b["type"] == kind)
    }

    /// The answer's text blocks, joined. With a schema, this is the JSON.
    pub fn text(&self) -> String {
        self.blocks("text")
            .filter_map(|b| b["text"].as_str())
            .collect()
    }

    /// The pages the answer's text cites, each once, in order, with every
    /// passage cited from it: a page is often cited for many facts, and a
    /// later step can only hold a fact to words it is shown.
    pub fn citations(&self) -> Vec<Citation> {
        let read = self.pages_read();
        let mut seen = Vec::<(Citation, usize)>::new();
        for citation in self
            .blocks("text")
            .filter_map(|b| b["citations"].as_array())
            .flatten()
        {
            let Some((url, title)) = cited_page(citation, &read) else {
                continue;
            };
            let words = passage(citation["cited_text"].as_str().unwrap_or(""));
            let words = words.as_str();
            match seen.iter_mut().find(|(c, _)| c.url == url) {
                Some((page, passages)) => {
                    if *passages < MAX_PASSAGES && !page.cited_text.contains(words) {
                        page.cited_text.push_str(" … ");
                        page.cited_text.push_str(words);
                        *passages += 1;
                    }
                }
                None => seen.push((
                    Citation {
                        url: url.to_owned(),
                        title: title.to_owned(),
                        cited_text: words.to_owned(),
                    },
                    1,
                )),
            }
        }
        seen.into_iter().map(|(c, _)| c).collect()
    }

    /// The pages read whole, in the order the answer's citations number
    /// them, with their titles, or their addresses for pages without one.
    /// A page that could not be read still takes its place in the order.
    fn pages_read(&self) -> Vec<Option<(&str, &str)>> {
        self.blocks("web_fetch_tool_result")
            .map(|b| {
                let result = &b["content"];
                let url = result["url"].as_str()?;
                let document = &result["content"];
                if result["type"] != "web_fetch_result" || document["type"] != "document" {
                    return None;
                }
                Some((url, document["title"].as_str().unwrap_or(url)))
            })
            .collect()
    }

    /// The answer's text with each cited passage followed by the numbers
    /// of the pages it cites, as [`citations`](Self::citations) numbers
    /// them from 1, so a later step can tell which page says what.
    pub fn marked_text(&self) -> String {
        let pages = self.citations();
        let read = self.pages_read();
        let mut text = String::new();
        for block in self.blocks("text") {
            text.push_str(block["text"].as_str().unwrap_or(""));
            let mut marks: Vec<usize> = block["citations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| cited_page(c, &read).map(|(url, _)| url))
                .filter_map(|url| pages.iter().position(|p| p.url == url))
                .collect();
            marks.sort_unstable();
            marks.dedup();
            for n in marks {
                text.push_str(&format!(" [{}]", n + 1));
            }
        }
        text
    }

    /// The error codes of searches that failed, such as
    /// `max_uses_exceeded`; they come inside a successful answer.
    pub fn search_errors(&self) -> Vec<String> {
        self.blocks("web_search_tool_result")
            .filter_map(|b| b["content"]["error_code"].as_str())
            .map(str::to_owned)
            .collect()
    }
}

/// The address and title of the page a citation cites: a search result
/// names its page, while a page read whole is cited by its place among
/// the pages read.
fn cited_page<'a>(
    citation: &'a Value,
    read: &[Option<(&'a str, &'a str)>],
) -> Option<(&'a str, &'a str)> {
    if let Some(url) = citation["url"].as_str() {
        return Some((url, citation["title"].as_str().unwrap_or("")));
    }
    if citation["type"] != "char_location" {
        return None;
    }
    let index = usize::try_from(citation["document_index"].as_u64()?).ok()?;
    *read.get(index)?
}

/// A cited passage, cut to a length a later step can be shown.
fn passage(words: &str) -> String {
    match words.char_indices().nth(MAX_PASSAGE_CHARS) {
        Some((end, _)) => format!("{}…", &words[..end]),
        None => words.to_owned(),
    }
}

/// A model the owner can pick, with what it can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub image_input: bool,
    pub structured_outputs: bool,
    pub web_search: bool,
}

#[derive(Deserialize)]
struct ModelPage {
    data: Vec<Value>,
    has_more: bool,
    last_id: Option<String>,
}

fn supported(model: &Value, path: &[&str]) -> bool {
    let mut at = &model["capabilities"];
    for key in path {
        at = &at[*key];
    }
    at["supported"].as_bool().unwrap_or(false)
}

/// A model from the list, if its id is one the app can use.
fn model_info(model: &Value) -> Option<ModelInfo> {
    let id = model["id"].as_str().filter(|id| plain_id(id))?;
    Some(ModelInfo {
        id: id.to_owned(),
        display_name: model["display_name"].as_str().unwrap_or(id).to_owned(),
        image_input: supported(model, &["image_input"]),
        structured_outputs: supported(model, &["structured_outputs"]),
        web_search: supported(model, &["server_tools", "web_search"]),
    })
}

/// A model id as Anthropic writes them, safe to put in a query.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
}

/// Anthropic's API, called with the owner's key.
pub struct Anthropic<'a> {
    client: &'a Client,
    key: &'a ApiKey,
}

impl<'a> Anthropic<'a> {
    pub fn new(client: &'a Client, key: &'a ApiKey) -> Self {
        Self { client, key }
    }

    fn headers(&self, json: bool) -> Vec<(&'static str, Zeroizing<String>)> {
        let mut headers = vec![
            ("x-api-key", Zeroizing::new(self.key.expose().to_owned())),
            ("anthropic-version", Zeroizing::new(API_VERSION.to_owned())),
        ];
        if json {
            headers.push((
                "content-type",
                Zeroizing::new("application/json".to_owned()),
            ));
        }
        headers
    }

    fn get(&self, path: String, cancel: &AtomicBool) -> Result<crate::http::Response, NetError> {
        let response = crate::retry::send(
            self.client,
            || Request {
                host: Host::Anthropic,
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
        Ok(response)
    }

    /// Every model the key can use, newest first.
    pub fn models(&self, cancel: &AtomicBool) -> Result<Vec<ModelInfo>, NetError> {
        let mut models = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..MAX_MODEL_PAGES {
            let path = match &after {
                Some(id) => format!("/v1/models?limit=1000&after_id={id}"),
                None => "/v1/models?limit=1000".to_owned(),
            };
            let page: ModelPage = parse(&self.get(path, cancel)?)?;
            models.extend(page.data.iter().filter_map(model_info));
            match page.last_id.filter(|id| page.has_more && plain_id(id)) {
                Some(id) => after = Some(id),
                None => break,
            }
        }
        Ok(models)
    }

    /// Sends one message and returns the answer, whatever its stop reason.
    pub fn message(
        &self,
        request: &MessageRequest,
        cancel: &AtomicBool,
    ) -> Result<Message, NetError> {
        let body = serde_json::to_vec(&request_body(request)?)
            .map_err(|_| NetError::Invalid("the request could not be written"))?;
        let response = crate::retry::send(
            self.client,
            || Request {
                host: Host::Anthropic,
                method: Method::Post,
                path: "/v1/messages".into(),
                headers: self.headers(true),
                body: Some(body.clone()),
            },
            cancel,
        )?;
        if response.status != 200 {
            return Err(status_error(&response, self.key.expose()));
        }
        Ok(Message::from_raw(parse(&response)?))
    }
}

/// The JSON body of a Messages request.
pub(crate) fn request_body(request: &MessageRequest) -> Result<Value, NetError> {
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
    if request.web_fetch.is_some() && request.web_search.is_none() {
        return Err(NetError::Invalid("pages are read only with a search"));
    }
    if request
        .web_fetch
        .as_ref()
        .is_some_and(|f| f.allowed_domains.is_empty())
    {
        return Err(NetError::Invalid("pages are read only from named sites"));
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
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/jpeg",
                        "data": crate::base64::encode(bytes),
                    }
                })
            }
        });
    }
    let mut messages = vec![json!({"role": "user", "content": content})];
    if !request.resume.is_empty() {
        messages.push(json!({"role": "assistant", "content": request.resume}));
    }
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "messages": messages,
    });
    if !request.system.is_empty() {
        body["system"] = json!(request.system);
    }
    if let Some(search) = &request.web_search {
        if !search.allowed_domains.is_empty() && !search.blocked_domains.is_empty() {
            return Err(NetError::Invalid(
                "a search may name allowed or blocked sites, not both",
            ));
        }
        let mut tool = json!({
            "type": WEB_SEARCH_TOOL,
            "name": "web_search",
            "max_uses": search.max_uses,
            // This version otherwise runs searches from code execution,
            // whose filtered results the answer cannot cite, and the
            // lookup uses only what is cited.
            "allowed_callers": ["direct"],
        });
        if !search.allowed_domains.is_empty() {
            tool["allowed_domains"] = json!(search.allowed_domains);
        }
        if !search.blocked_domains.is_empty() {
            tool["blocked_domains"] = json!(search.blocked_domains);
        }
        let mut tools = vec![tool];
        if let Some(fetch) = &request.web_fetch {
            tools.push(json!({
                "type": WEB_FETCH_TOOL,
                "name": "web_fetch",
                "max_uses": fetch.max_uses,
                "allowed_domains": fetch.allowed_domains,
                "max_content_tokens": fetch.max_content_tokens,
                "citations": {"enabled": true},
                // As for search: what code execution reads cannot be cited.
                "allowed_callers": ["direct"],
            }));
        }
        body["tools"] = json!(tools);
    }
    let mut output = serde_json::Map::new();
    if let Some(effort) = request.effort {
        output.insert("effort".into(), json!(effort.code()));
    }
    if let Some(schema) = &request.schema {
        output.insert(
            "format".into(),
            json!({"type": "json_schema", "schema": schema.schema}),
        );
    }
    if !output.is_empty() {
        body["output_config"] = Value::Object(output);
    }
    Ok(body)
}

#[cfg(test)]
mod tests;
