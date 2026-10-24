use serde_json::json;

use super::*;

const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];

fn request() -> ChatRequest {
    ChatRequest {
        model: "anthropic/claude-opus-5.5".into(),
        max_tokens: 16_000,
        system: "Read the cover.".into(),
        user: vec![Part::Jpeg(JPEG.to_vec()), Part::Text("Front cover.".into())],
        web_search: None,
        schema: None,
    }
}

fn ok(body: Value) -> Response {
    Response {
        status: 200,
        retry_after: None,
        body: serde_json::to_vec(&body).unwrap(),
    }
}

#[test]
fn photos_go_as_data_urls_after_the_instructions() {
    let body = request_body(&request()).unwrap();
    assert_eq!(body["model"], "anthropic/claude-opus-5.5");
    assert_eq!(
        body["messages"][0],
        json!({"role": "system", "content": "Read the cover."})
    );
    assert_eq!(
        body["messages"][1]["content"][0],
        json!({"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,/9j/4AAQ"}})
    );
    assert_eq!(
        body["messages"][1]["content"][1],
        json!({"type": "text", "text": "Front cover."})
    );
    assert_eq!(
        body["provider"],
        json!({"require_parameters": true, "data_collection": "deny"})
    );
    for name in ["temperature", "top_p", "top_k", "plugins"] {
        assert!(body.get(name).is_none(), "{name}");
    }
}

#[test]
fn search_and_schema_take_their_documented_forms() {
    let mut search = request();
    search.web_search = Some(WebSearch {
        max_results: 5,
        max_uses: 3,
    });
    assert_eq!(
        request_body(&search).unwrap()["tools"],
        json!([{"type": "openrouter:web_search", "parameters": {"max_results": 5, "max_uses": 3}}])
    );
    let mut structured = request();
    structured.schema = Some(Schema {
        name: "book",
        schema: json!({"type": "object"}),
    });
    assert_eq!(
        request_body(&structured).unwrap()["response_format"],
        json!({"type": "json_schema", "json_schema": {"name": "book", "strict": true, "schema": {"type": "object"}}})
    );
    structured.web_search = Some(WebSearch::default());
    assert!(matches!(
        request_body(&structured),
        Err(NetError::Invalid(_))
    ));
}

#[test]
fn an_answer_gives_its_text_sources_and_cost() {
    let response = ok(json!({
        "id": "gen-test",
        "model": "anthropic/claude-opus-5.5",
        "choices": [{
            "finish_reason": "stop",
            "native_finish_reason": "end_turn",
            "message": {
                "role": "assistant",
                "content": "The cover date is March 1954.",
                "annotations": [
                    {"type": "url_citation", "url_citation": {"url": "https://example.org/robot-tales-1", "title": "Robot Tales #1", "content": "Cover date: March 1954", "start_index": 0, "end_index": 10}},
                    {"type": "url_citation", "url_citation": {"url": "https://example.org/robot-tales-1", "title": "Robot Tales #1"}}
                ]
            }
        }],
        "usage": {"prompt_tokens": 1200, "completion_tokens": 80, "total_tokens": 1280, "cost": 0.00123, "server_tool_use": {"web_search_requests": 1}}
    }));
    let answer = answer(&response, "k").unwrap();
    assert_eq!(answer.text, "The cover date is March 1954.");
    assert_eq!(answer.finish, Finish::Stop);
    assert_eq!(answer.citations.len(), 1);
    assert_eq!(answer.citations[0].cited_text, "Cover date: March 1954");
    assert_eq!(
        answer.usage,
        Usage {
            prompt_tokens: 1200,
            completion_tokens: 80,
            web_searches: 1,
            cost: Some(Picodollars(1_230_000_000)),
        }
    );
}

#[test]
fn an_error_inside_a_200_answer_is_an_error() {
    let late = ok(json!({"error": {"code": 502, "message": "Provider returned error"}}));
    assert!(matches!(
        answer(&late, "k"),
        Err(NetError::Status { status: 200, kind, .. }) if kind == "502"
    ));
    let in_choice = ok(json!({
        "id": "gen-test", "model": "m",
        "choices": [{"finish_reason": "error", "error": {"code": 500, "message": "boom"}, "message": {"content": ""}}]
    }));
    assert!(matches!(
        answer(&in_choice, "k"),
        Err(NetError::Status { .. })
    ));
    assert!(matches!(
        answer(&ok(json!({"id": "x", "choices": []})), "k"),
        Err(NetError::BadResponse(_))
    ));
}

#[test]
fn cut_and_declined_answers_are_told_apart() {
    let finished = |reason: &str| {
        ok(
            json!({"id": "x", "model": "m", "choices": [{"finish_reason": reason, "message": {"content": "{"}}]}),
        )
    };
    assert_eq!(
        answer(&finished("length"), "k").unwrap().finish,
        Finish::Length
    );
    assert_eq!(
        answer(&finished("content_filter"), "k").unwrap().finish,
        Finish::ContentFilter
    );
}

#[test]
fn models_are_read_with_exact_prices() {
    let model = json!({
        "id": "anthropic/claude-opus-5.5",
        "name": "Anthropic: Claude Opus 5.5",
        "architecture": {"input_modalities": ["text", "image"], "output_modalities": ["text"]},
        "supported_parameters": ["max_tokens", "response_format", "structured_outputs", "tools"],
        "pricing": {"prompt": "0.000004", "completion": "0.00002", "web_search": "0.01", "image": "0"},
        "context_length": 1000000
    });
    let info = model_info(&model).unwrap();
    assert!(info.image_input && info.structured_outputs && info.tools);
    assert_eq!(info.prompt_price, Some(Picodollars(4_000_000)));
    assert_eq!(info.completion_price, Some(Picodollars(20_000_000)));
    assert_eq!(info.web_search_price, Some(Picodollars(10_000_000_000)));
    assert_eq!(info.image_price, Some(Picodollars(0)));
    assert!(model_info(&json!({"id": "bad id; rm -rf"})).is_none());
    let text_only = json!({"id": "x/y", "architecture": {"input_modalities": ["text"]}, "supported_parameters": []});
    let info = model_info(&text_only).unwrap();
    assert!(!info.image_input && !info.structured_outputs && info.prompt_price.is_none());
}

#[test]
fn the_key_goes_only_in_its_header() {
    let client = Client::new();
    let key = ApiKey::new("sk-or-v1-TESTONLY".into()).unwrap();
    let api = OpenRouter::new(&client, &key);
    let headers = api.headers(true);
    assert_eq!(headers[0].0, "authorization");
    assert_eq!(headers[0].1.as_str(), "Bearer sk-or-v1-TESTONLY");
    assert!(
        !request_body(&request())
            .unwrap()
            .to_string()
            .contains("TESTONLY")
    );
}
