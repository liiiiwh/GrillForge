use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use grillforge_lib::bridge::{
    GeminiNativeBridge, anthropic_response_to_gemini, gemini_request_to_anthropic,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use url::Url;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<(HeaderMap, Value)>>>);

#[tokio::test]
async fn empty_claude_tool_list_is_omitted_from_gemini_request() {
    let capture = Capture::default();
    let upstream =
        Router::new()
            .route(
                "/v1beta/models/gemini-2.5-pro:generateContent",
                post(
                    |State(capture): State<Capture>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        capture.0.lock().unwrap().push((headers, body));
                        Json(json!({
                            "responseId":"gemini-empty-tools",
                            "modelVersion":"gemini-2.5-pro",
                            "candidates":[{
                                "finishReason":"STOP",
                                "content":{"role":"model","parts":[{"text":"pong"}]}
                            }],
                            "usageMetadata":{"promptTokenCount":3,"totalTokenCount":4}
                        }))
                    },
                ),
            )
            .with_state(capture.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let endpoint = Url::parse(&format!(
        "http://{address}/v1beta/models/gemini-2.5-pro:generateContent"
    ))
    .unwrap();

    GeminiNativeBridge::from_endpoint(endpoint, "gemini-secret")
        .complete(json!({
            "model":"gemini-2.5-pro","max_tokens":128,
            "messages":[{"role":"user","content":"ping"}],
            "tools":[]
        }))
        .await
        .expect("an empty Claude tool list is valid");

    let calls = capture.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.get("tools").is_none());
}

#[test]
fn gemini_client_request_and_anthropic_response_round_trip_tools() {
    let anthropic = gemini_request_to_anthropic(
        "grillforge/coder",
        json!({
            "systemInstruction":{"parts":[{"text":"Be concise."}]},
            "contents":[
                {"role":"user","parts":[{"text":"Weather?"}]},
                {"role":"model","parts":[{"functionCall":{"id":"call_1","name":"weather","args":{"city":"Paris"}}}]},
                {"role":"user","parts":[{"functionResponse":{"id":"call_1","name":"weather","response":{"temperature":20}}}]}
            ],
            "generationConfig":{"maxOutputTokens":256,"temperature":0.2,"stopSequences":["END"]},
            "tools":[{"functionDeclarations":[{
                "name":"weather","description":"Current weather",
                "parametersJsonSchema":{"type":"object","properties":{"city":{"type":"string"}}}
            }]}],
            "toolConfig":{"functionCallingConfig":{"mode":"AUTO"}}
        }),
        false,
    )
    .expect("Gemini client request");

    assert_eq!(anthropic["model"], "grillforge/coder");
    assert_eq!(anthropic["system"], "Be concise.");
    assert_eq!(anthropic["max_tokens"], 256);
    assert_eq!(anthropic["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(
        anthropic["messages"][2]["content"][0]["tool_use_id"],
        "call_1"
    );
    assert_eq!(anthropic["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(anthropic["tool_choice"]["type"], "auto");

    let gemini = anthropic_response_to_gemini(json!({
        "id":"msg_1","type":"message","role":"assistant","model":"coder-upstream",
        "content":[
            {"type":"text","text":"Calling weather."},
            {"type":"tool_use","id":"call_2","name":"weather","input":{"city":"Tokyo"}}
        ],
        "stop_reason":"tool_use","stop_sequence":null,
        "usage":{"input_tokens":12,"cache_read_input_tokens":3,"output_tokens":4}
    }))
    .expect("Anthropic response");

    assert_eq!(gemini["responseId"], "msg_1");
    assert_eq!(gemini["candidates"][0]["content"]["role"], "model");
    assert_eq!(
        gemini["candidates"][0]["content"]["parts"][0]["text"],
        "Calling weather."
    );
    assert_eq!(
        gemini["candidates"][0]["content"]["parts"][1]["functionCall"]["id"],
        "call_2"
    );
    assert_eq!(gemini["usageMetadata"]["promptTokenCount"], 15);
    assert_eq!(gemini["usageMetadata"]["candidatesTokenCount"], 4);
    assert_eq!(gemini["usageMetadata"]["totalTokenCount"], 19);
}

#[tokio::test]
async fn anthropic_text_round_trips_through_gemini_native_with_usage() {
    let capture = Capture::default();
    let upstream =
        Router::new()
            .route(
                "/v1beta/models/gemini-2.5-pro:generateContent",
                post(
                    |State(capture): State<Capture>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        capture.0.lock().unwrap().push((headers, body));
                        Json(json!({
                            "responseId":"gemini-response-1",
                            "modelVersion":"gemini-2.5-pro",
                            "candidates":[{
                                "finishReason":"STOP",
                                "content":{"role":"model","parts":[{"text":"hello from Gemini"}]}
                            }],
                            "usageMetadata":{
                                "promptTokenCount":12,
                                "cachedContentTokenCount":2,
                                "totalTokenCount":17
                            }
                        }))
                    },
                ),
            )
            .with_state(capture.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let endpoint = Url::parse(&format!(
        "http://{address}/v1beta/models/gemini-2.5-pro:generateContent"
    ))
    .unwrap();
    let bridge = GeminiNativeBridge::from_endpoint(endpoint, "gemini-secret");

    let response = bridge
        .complete(json!({
            "model":"gemini-2.5-pro",
            "max_tokens":128,
            "system":"Be concise.",
            "messages":[{"role":"user","content":"hello"}]
        }))
        .await
        .expect("Gemini bridge response");

    let calls = capture.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0["x-goog-api-key"], "gemini-secret");
    assert_eq!(
        calls[0].1["systemInstruction"]["parts"][0]["text"],
        "Be concise."
    );
    assert_eq!(calls[0].1["contents"][0]["parts"][0]["text"], "hello");
    assert_eq!(calls[0].1["generationConfig"]["maxOutputTokens"], 128);
    assert_eq!(response["content"][0]["text"], "hello from Gemini");
    assert_eq!(response["stop_reason"], "end_turn");
    assert_eq!(response["usage"]["input_tokens"], 10);
    assert_eq!(response["usage"]["cache_read_input_tokens"], 2);
    assert_eq!(response["usage"]["output_tokens"], 5);
}

#[tokio::test]
async fn anthropic_tool_history_and_result_round_trip_through_gemini() {
    let capture = Capture::default();
    let upstream = Router::new()
        .route(
            "/v1beta/models/gemini-2.5-pro:generateContent",
            post(
                |State(capture): State<Capture>, headers: HeaderMap, Json(body): Json<Value>| async move {
                    capture.0.lock().unwrap().push((headers, body));
                    Json(json!({
                        "responseId":"gemini-tools",
                        "modelVersion":"gemini-2.5-pro",
                        "candidates":[{
                            "finishReason":"STOP",
                            "content":{"role":"model","parts":[{
                                "functionCall":{"id":"call_2","name":"forecast","args":{"city":"Paris"}}
                            }]}
                        }],
                        "usageMetadata":{"promptTokenCount":20,"totalTokenCount":24}
                    }))
                },
            ),
        )
        .with_state(capture.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let endpoint = Url::parse(&format!(
        "http://{address}/v1beta/models/gemini-2.5-pro:generateContent"
    ))
    .unwrap();
    let bridge = GeminiNativeBridge::from_endpoint(endpoint, "gemini-secret");

    let response = bridge
        .complete(json!({
            "model":"gemini-2.5-pro",
            "max_tokens":128,
            "messages":[
                {"role":"user","content":"weather?"},
                {"role":"assistant","content":[{
                    "type":"tool_use","id":"call_1","name":"forecast","input":{"city":"Tokyo"}
                }]},
                {"role":"user","content":[{
                    "type":"tool_result","tool_use_id":"call_1","content":"sunny"
                }]}
            ],
            "tools":[{
                "name":"forecast",
                "description":"Get weather",
                "input_schema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}
            }],
            "tool_choice":{"type":"auto"}
        }))
        .await
        .expect("Gemini tool response");

    let calls = capture.0.lock().unwrap();
    assert_eq!(
        calls[0].1["contents"][1]["parts"][0]["functionCall"]["id"],
        "call_1"
    );
    assert_eq!(
        calls[0].1["contents"][2]["parts"][0]["functionResponse"]["name"],
        "forecast"
    );
    assert_eq!(
        calls[0].1["contents"][2]["parts"][0]["functionResponse"]["response"]["content"],
        "sunny"
    );
    assert_eq!(
        calls[0].1["tools"][0]["functionDeclarations"][0]["name"],
        "forecast"
    );
    assert_eq!(
        calls[0].1["toolConfig"]["functionCallingConfig"]["mode"],
        "AUTO"
    );
    assert_eq!(response["content"][0]["type"], "tool_use");
    assert_eq!(response["content"][0]["id"], "call_2");
    assert_eq!(response["content"][0]["name"], "forecast");
    assert_eq!(response["content"][0]["input"]["city"], "Paris");
    assert_eq!(response["stop_reason"], "tool_use");
}

#[tokio::test]
async fn anthropic_base64_image_becomes_gemini_inline_data() {
    let capture = Capture::default();
    let upstream = Router::new()
        .route(
            "/v1beta/models/gemini-2.5-pro:generateContent",
            post(
                |State(capture): State<Capture>, headers: HeaderMap, Json(body): Json<Value>| async move {
                    capture.0.lock().unwrap().push((headers, body));
                    Json(json!({
                        "responseId":"gemini-image",
                        "modelVersion":"gemini-2.5-pro",
                        "candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"image received"}]}}]
                    }))
                },
            ),
        )
        .with_state(capture.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let endpoint = Url::parse(&format!(
        "http://{address}/v1beta/models/gemini-2.5-pro:generateContent"
    ))
    .unwrap();
    let bridge = GeminiNativeBridge::from_endpoint(endpoint, "gemini-secret");

    bridge
        .complete(json!({
            "model":"gemini-2.5-pro",
            "max_tokens":32,
            "messages":[{"role":"user","content":[
                {"type":"text","text":"describe"},
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}}
            ]}]
        }))
        .await
        .expect("Gemini image response");

    let calls = capture.0.lock().unwrap();
    assert_eq!(
        calls[0].1["contents"][0]["parts"][1],
        json!({"inlineData":{"mimeType":"image/png","data":"aGVsbG8="}})
    );
}

#[tokio::test]
async fn unsupported_anthropic_block_fails_before_upstream_io() {
    let endpoint = Url::parse("http://127.0.0.1:1/v1beta/models/gemini:generateContent").unwrap();
    let bridge = GeminiNativeBridge::from_endpoint(endpoint, "never-print");

    let error = bridge
        .complete(json!({
            "model":"gemini",
            "max_tokens":32,
            "messages":[{"role":"user","content":[{
                "type":"document","source":{"type":"base64","media_type":"application/pdf","data":"AA=="}
            }]}]
        }))
        .await
        .expect_err("unsupported block must fail locally");

    assert_eq!(
        error.to_string(),
        "invalid Anthropic request for Gemini: messages[0].content[0] block type is unsupported: document"
    );
    assert!(!error.to_string().contains("never-print"));
}

#[tokio::test]
async fn gemini_error_preserves_status_and_safe_context_without_credentials() {
    let upstream = Router::new().route(
        "/v1beta/models/gemini:generateContent",
        post(|| async {
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error":{
                        "status":"RESOURCE_EXHAUSTED",
                        "message":"quota for gemini-secret\ntry later"
                    }
                })),
            )
                .into_response()
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let endpoint = Url::parse(&format!(
        "http://{address}/v1beta/models/gemini:generateContent"
    ))
    .unwrap();
    let bridge = GeminiNativeBridge::from_endpoint(endpoint, "gemini-secret");

    let error = bridge
        .complete(json!({
            "model":"gemini",
            "max_tokens":16,
            "messages":[{"role":"user","content":"hello"}]
        }))
        .await
        .expect_err("Gemini quota error");

    assert_eq!(error.upstream_http_status(), Some(429));
    assert_eq!(
        error.to_string(),
        "Gemini upstream returned HTTP 429 (RESOURCE_EXHAUSTED): quota for [redacted] try later"
    );
}
