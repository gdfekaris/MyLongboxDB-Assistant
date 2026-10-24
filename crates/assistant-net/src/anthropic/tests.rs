use serde_json::json;

use super::*;
use crate::http::Response;

const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];

fn request() -> MessageRequest {
    MessageRequest {
        model: DEFAULT_MODEL.into(),
        max_tokens: 16_000,
        system: "Read the cover.".into(),
        user: vec![Part::Jpeg(JPEG.to_vec()), Part::Text("Front cover.".into())],
        resume: Vec::new(),
        web_search: None,
        schema: None,
        effort: None,
    }
}

fn parsed(body: Value) -> Message {
    let response = Response {
        status: 200,
        retry_after: None,
        body: serde_json::to_vec(&body).unwrap(),
    };
    Message::from_raw(parse(&response).unwrap())
}

#[test]
fn photos_go_as_base64_jpeg_blocks_in_order() {
    let body = request_body(&request()).unwrap();
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["max_tokens"], 16_000);
    assert_eq!(body["system"], "Read the cover.");
    let content = &body["messages"][0]["content"];
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(
        content[0],
        json!({"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "/9j/4AAQ"}})
    );
    assert_eq!(content[1], json!({"type": "text", "text": "Front cover."}));
    assert!(body.get("tools").is_none());
    assert!(body.get("output_config").is_none());
}

#[test]
fn sampling_settings_are_never_sent() {
    let text = request_body(&request()).unwrap().to_string();
    for name in ["temperature", "top_p", "top_k", "thinking", "budget_tokens"] {
        assert!(!text.contains(name), "{name}");
    }
}

#[test]
fn web_search_is_the_server_tool_with_its_limits() {
    let mut search = request();
    search.web_search = Some(WebSearch {
        max_uses: 5,
        allowed_domains: vec!["comics.org".into()],
        blocked_domains: Vec::new(),
    });
    search.effort = Some(Effort::High);
    let body = request_body(&search).unwrap();
    assert_eq!(
        body["tools"],
        json!([{"type": "web_search_20260209", "name": "web_search", "max_uses": 5, "allowed_domains": ["comics.org"]}])
    );
    assert_eq!(body["output_config"], json!({"effort": "high"}));
}

#[test]
fn a_schema_goes_in_output_config() {
    let mut structured = request();
    structured.schema = Some(Schema {
        name: "book",
        schema: json!({"type": "object", "properties": {}, "required": [], "additionalProperties": false}),
    });
    let body = request_body(&structured).unwrap();
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(
        body["output_config"]["format"]["schema"]["additionalProperties"],
        false
    );
}

#[test]
fn a_paused_answer_is_sent_back_unchanged() {
    let mut resumed = request();
    let paused = vec![
        json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "q"}}),
    ];
    resumed.resume = paused.clone();
    let body = request_body(&resumed).unwrap();
    assert_eq!(
        body["messages"][1],
        json!({"role": "assistant", "content": paused})
    );
}

#[test]
fn requests_the_app_must_not_make_are_refused() {
    let mut both = request();
    both.schema = Some(Schema {
        name: "book",
        schema: json!({}),
    });
    both.web_search = Some(WebSearch::default());
    assert!(matches!(request_body(&both), Err(NetError::Invalid(_))));

    let mut domains = request();
    domains.web_search = Some(WebSearch {
        max_uses: 1,
        allowed_domains: vec!["a.example".into()],
        blocked_domains: vec!["b.example".into()],
    });
    assert!(matches!(request_body(&domains), Err(NetError::Invalid(_))));

    let mut png = request();
    png.user = vec![Part::Jpeg(b"\x89PNG\r\n".to_vec())];
    assert!(matches!(request_body(&png), Err(NetError::Invalid(_))));

    let mut empty = request();
    empty.user.clear();
    assert!(matches!(request_body(&empty), Err(NetError::Invalid(_))));
}

/// Shaped like the documented web search answer; the content is made up.
fn searched() -> Value {
    json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5-5",
        "content": [
            {"type": "text", "text": "Searching."},
            {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "astonishing robot tales 1"}},
            {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [
                {"type": "web_search_result", "url": "https://example.org/robot-tales-1", "title": "Robot Tales #1", "encrypted_content": "Zm9v", "page_age": "May 1, 2026"}
            ]},
            {"type": "server_tool_use", "id": "srvtoolu_2", "name": "web_search", "input": {"query": "q"}},
            {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_2", "content": {"type": "web_search_tool_result_error", "error_code": "max_uses_exceeded"}},
            {"type": "text", "text": "The cover date is March 1954.", "citations": [
                {"type": "web_search_result_location", "url": "https://example.org/robot-tales-1", "title": "Robot Tales #1", "encrypted_index": "Zm9v", "cited_text": "Cover date: March 1954"},
                {"type": "web_search_result_location", "url": "https://example.org/robot-tales-1", "title": "Robot Tales #1", "encrypted_index": "YmFy", "cited_text": "1954"}
            ]}
        ],
        "stop_reason": "end_turn",
        "stop_details": null,
        "usage": {"input_tokens": 6039, "output_tokens": 931, "server_tool_use": {"web_search_requests": 2}}
    })
}

#[test]
fn a_searched_answer_gives_its_text_sources_and_cost() {
    let message = parsed(searched());
    assert_eq!(message.stop, Stop::EndTurn);
    assert_eq!(message.text(), "Searching.The cover date is March 1954.");
    assert_eq!(
        message.citations(),
        vec![Citation {
            url: "https://example.org/robot-tales-1".into(),
            title: "Robot Tales #1".into(),
            cited_text: "Cover date: March 1954".into(),
        }]
    );
    assert_eq!(message.search_errors(), vec!["max_uses_exceeded"]);
    assert_eq!(
        message.usage,
        Usage {
            input_tokens: 6039,
            output_tokens: 931,
            web_searches: 2,
            ..Usage::default()
        }
    );
}

#[test]
fn refusals_and_pauses_are_told_apart() {
    let mut refused = searched();
    refused["stop_reason"] = json!("refusal");
    refused["stop_details"] = json!({"type": "refusal", "category": "cyber", "explanation": "x"});
    assert_eq!(
        parsed(refused).stop,
        Stop::Refusal {
            category: Some("cyber".into())
        }
    );
    let mut paused = searched();
    paused["stop_reason"] = json!("pause_turn");
    assert_eq!(parsed(paused).stop, Stop::PauseTurn);
    let mut cut = searched();
    cut["stop_reason"] = json!("max_tokens");
    assert_eq!(parsed(cut).stop, Stop::MaxTokens);
}

#[test]
fn an_answer_missing_its_parts_is_not_read() {
    let response = Response {
        status: 200,
        retry_after: None,
        body: br#"{"id": "msg_test", "content": []}"#.to_vec(),
    };
    assert!(matches!(
        parse::<RawMessage>(&response),
        Err(NetError::BadResponse(_))
    ));
}

#[test]
fn models_are_read_with_what_they_can_do() {
    let yes = json!({"supported": true});
    let no = json!({"supported": false});
    let page = json!({
        "data": [
            {"id": "claude-opus-5-5", "display_name": "Claude Opus 5.5", "type": "model",
             "capabilities": {"image_input": yes, "structured_outputs": yes,
                              "server_tools": {"supported": true, "web_search": yes, "code_execution": yes}}},
            {"id": "claude-haiku-4-5", "display_name": "Claude Haiku 4.5", "type": "model",
             "capabilities": {"image_input": yes, "structured_outputs": yes,
                              "server_tools": {"supported": true, "web_search": no}}},
            {"id": "bad id/../", "display_name": "x", "type": "model"}
        ],
        "has_more": false,
        "first_id": "claude-opus-5-5",
        "last_id": "bad id/../"
    });
    let page: ModelPage = serde_json::from_value(page).unwrap();
    let models: Vec<ModelInfo> = page.data.iter().filter_map(model_info).collect();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "claude-opus-5-5");
    assert!(models[0].image_input && models[0].structured_outputs && models[0].web_search);
    assert!(!models[1].web_search);
}

#[test]
fn the_key_goes_only_in_its_header() {
    let client = Client::new();
    let key = ApiKey::new("sk-ant-api03-TESTONLY".into()).unwrap();
    let api = Anthropic::new(&client, &key);
    let headers = api.headers(true);
    let names: Vec<&str> = headers.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, ["x-api-key", "anthropic-version", "content-type"]);
    assert_eq!(headers[1].1.as_str(), "2026-10-21");
    let body = request_body(&request()).unwrap().to_string();
    assert!(!body.contains("TESTONLY"));
}
