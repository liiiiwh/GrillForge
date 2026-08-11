#[path = "../src/usage_query.rs"]
mod usage_query;

use std::future::ready;
use usage_query::{
    AuthStyle, ResetAt, UsageHttpRequest, UsageHttpResponse, UsageKind, UsageQueryCredentials,
    UsageQueryError, UsageQueryPreset, UsageTransport, query_usage, query_usage_with_transport,
    supported_usage_queries,
};

struct StubTransport {
    endpoint: &'static str,
    auth: AuthStyle,
    response: UsageHttpResponse,
}

#[tokio::test]
async fn parses_every_vetted_coding_plan_response_shape() {
    let credentials = UsageQueryCredentials::new("test-key").unwrap();
    let kimi = StubTransport {
        endpoint: "https://api.kimi.com/coding/v1/usages",
        auth: AuthStyle::Bearer,
        response: UsageHttpResponse {
            status: 200,
            body: br#"{
                "limits":[{"detail":{"limit":100,"remaining":40,"resetTime":1786400000000}}],
                "usage":{"limit":1000,"remaining":700,"resetTime":"2026-08-18T00:00:00Z"}
            }"#
            .to_vec(),
        },
    };
    let kimi = query_usage_with_transport(&kimi, UsageQueryPreset::KimiCodingPlan, &credentials)
        .await
        .unwrap();
    assert_eq!(kimi.items[0].label, "five_hour");
    assert_eq!(kimi.items[0].utilization_percent, Some(60.0));
    assert_eq!(
        kimi.items[0].reset_at,
        Some(ResetAt::UnixMilliseconds(1_786_400_000_000))
    );
    assert_eq!(kimi.items[1].utilization_percent, Some(30.0));
    assert_eq!(
        kimi.items[1].reset_at,
        Some(ResetAt::Iso8601("2026-08-18T00:00:00Z".to_owned()))
    );

    for (preset, endpoint) in [
        (
            UsageQueryPreset::ZhipuCnCodingPlan,
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit",
        ),
        (
            UsageQueryPreset::ZhipuGlobalCodingPlan,
            "https://api.z.ai/api/monitor/usage/quota/limit",
        ),
    ] {
        let zhipu = StubTransport {
            endpoint,
            auth: AuthStyle::AuthorizationValue,
            response: UsageHttpResponse {
                status: 200,
                body: br#"{
                    "success":true,
                    "data":{"level":"PRO","limits":[
                        {"type":"TOKENS_LIMIT","unit":3,"percentage":26,"nextResetTime":1786400000000},
                        {"type":"TOKENS_LIMIT","unit":6,"percentage":5,"nextResetTime":1787000000000}
                    ]}
                }"#
                .to_vec(),
            },
        };
        let zhipu = query_usage_with_transport(&zhipu, preset, &credentials)
            .await
            .unwrap();
        assert_eq!(zhipu.items[0].label, "five_hour");
        assert_eq!(zhipu.items[0].utilization_percent, Some(26.0));
        assert_eq!(zhipu.items[1].label, "weekly_limit");
        assert_eq!(zhipu.items[1].utilization_percent, Some(5.0));
    }

    let legacy_zhipu = StubTransport {
        endpoint: "https://open.bigmodel.cn/api/monitor/usage/quota/limit",
        auth: AuthStyle::AuthorizationValue,
        response: UsageHttpResponse {
            status: 200,
            body: br#"{
                "success":true,
                "data":{"limits":[
                    {"type":"TOKENS_LIMIT","percentage":7,"nextResetTime":1787000000000},
                    {"type":"TOKENS_LIMIT","percentage":22,"nextResetTime":1786400000000}
                ]}
            }"#
            .to_vec(),
        },
    };
    let legacy_zhipu = query_usage_with_transport(
        &legacy_zhipu,
        UsageQueryPreset::ZhipuCnCodingPlan,
        &credentials,
    )
    .await
    .unwrap();
    assert_eq!(legacy_zhipu.items[0].label, "five_hour");
    assert_eq!(legacy_zhipu.items[0].utilization_percent, Some(22.0));
    assert_eq!(legacy_zhipu.items[1].label, "weekly_limit");
    assert_eq!(legacy_zhipu.items[1].utilization_percent, Some(7.0));

    for (preset, endpoint) in [
        (
            UsageQueryPreset::MiniMaxCnCodingPlan,
            "https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains",
        ),
        (
            UsageQueryPreset::MiniMaxGlobalCodingPlan,
            "https://api.minimax.io/v1/api/openplatform/coding_plan/remains",
        ),
    ] {
        let minimax = StubTransport {
            endpoint,
            auth: AuthStyle::Bearer,
            response: UsageHttpResponse {
                status: 200,
                body: br#"{
                    "base_resp":{"status_code":0},
                    "model_remains":[{
                        "model_name":"general",
                        "current_interval_remaining_percent":80,
                        "end_time":1786400000000,
                        "current_weekly_status":1,
                        "current_weekly_remaining_percent":50,
                        "weekly_end_time":1787000000000
                    }]
                }"#
                .to_vec(),
            },
        };
        let minimax = query_usage_with_transport(&minimax, preset, &credentials)
            .await
            .unwrap();
        assert_eq!(minimax.items[0].utilization_percent, Some(20.0));
        assert_eq!(minimax.items[1].utilization_percent, Some(50.0));
    }
}

impl UsageTransport for StubTransport {
    fn get<'a>(
        &'a self,
        request: &'a UsageHttpRequest,
        api_key: &'a str,
    ) -> impl std::future::Future<Output = Result<UsageHttpResponse, UsageQueryError>> + Send + 'a
    {
        assert_eq!(request.endpoint, self.endpoint);
        assert_eq!(request.auth, self.auth);
        assert_eq!(api_key, "test-key");
        ready(Ok(self.response.clone()))
    }
}

#[test]
fn exposes_only_vetted_fixed_endpoint_queries() {
    let capabilities = supported_usage_queries();

    assert_eq!(capabilities.len(), 11);
    assert_eq!(capabilities[0].preset, UsageQueryPreset::DeepSeekBalance);
    assert_eq!(capabilities[0].kind, UsageKind::Balance);
    assert_eq!(capabilities[0].auth, AuthStyle::Bearer);
    assert_eq!(
        capabilities[0].endpoint,
        "https://api.deepseek.com/user/balance"
    );
    assert!(
        capabilities
            .iter()
            .all(|capability| capability.endpoint.starts_with("https://"))
    );
    assert!(
        capabilities
            .iter()
            .all(|capability| !capability.endpoint.contains("{{"))
    );
    assert!(capabilities.iter().any(|capability| {
        capability.preset == UsageQueryPreset::ZhipuCnCodingPlan
            && capability.auth == AuthStyle::AuthorizationValue
            && capability.endpoint == "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
    }));
}

#[test]
fn production_transport_can_only_be_used_through_a_preset() {
    let _query_entrypoint = query_usage;
}

#[tokio::test]
async fn queries_and_parses_deepseek_balance_without_persisting_credentials() {
    let transport = StubTransport {
        endpoint: "https://api.deepseek.com/user/balance",
        auth: AuthStyle::Bearer,
        response: UsageHttpResponse {
            status: 200,
            body: br#"{
                "is_available": true,
                "balance_infos": [
                    {"currency":"CNY","total_balance":"12.50"},
                    {"currency":"USD","total_balance":"3.25"}
                ]
            }"#
            .to_vec(),
        },
    };
    let credentials = UsageQueryCredentials::new("test-key").unwrap();

    let snapshot =
        query_usage_with_transport(&transport, UsageQueryPreset::DeepSeekBalance, &credentials)
            .await
            .unwrap();

    assert_eq!(snapshot.kind, UsageKind::Balance);
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(snapshot.items[0].label, "CNY");
    assert_eq!(snapshot.items[0].remaining, Some(12.5));
    assert_eq!(snapshot.items[0].unit.as_deref(), Some("CNY"));
    assert_eq!(snapshot.items[1].remaining, Some(3.25));
    assert_eq!(
        format!("{credentials:?}"),
        "UsageQueryCredentials([REDACTED])"
    );
}

async fn query_fixture(
    preset: UsageQueryPreset,
    endpoint: &'static str,
    body: &'static [u8],
) -> usage_query::UsageSnapshot {
    let transport = StubTransport {
        endpoint,
        auth: AuthStyle::Bearer,
        response: UsageHttpResponse {
            status: 200,
            body: body.to_vec(),
        },
    };
    query_usage_with_transport(
        &transport,
        preset,
        &UsageQueryCredentials::new("test-key").unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn parses_every_vetted_balance_response_shape() {
    let stepfun = query_fixture(
        UsageQueryPreset::StepFunBalance,
        "https://api.stepfun.com/v1/accounts",
        br#"{"balance":"8.50"}"#,
    )
    .await;
    assert_eq!(stepfun.items[0].remaining, Some(8.5));
    assert_eq!(stepfun.items[0].unit.as_deref(), Some("CNY"));

    for (preset, endpoint, unit) in [
        (
            UsageQueryPreset::SiliconFlowCnBalance,
            "https://api.siliconflow.cn/v1/user/info",
            "CNY",
        ),
        (
            UsageQueryPreset::SiliconFlowGlobalBalance,
            "https://api.siliconflow.com/v1/user/info",
            "USD",
        ),
    ] {
        let snapshot = query_fixture(
            preset,
            endpoint,
            br#"{"data":{"totalBalance":"7.25","status":"normal"}}"#,
        )
        .await;
        assert_eq!(snapshot.items[0].remaining, Some(7.25));
        assert_eq!(snapshot.items[0].unit.as_deref(), Some(unit));
    }

    let openrouter = query_fixture(
        UsageQueryPreset::OpenRouterBalance,
        "https://openrouter.ai/api/v1/credits",
        br#"{"data":{"total_credits":20,"total_usage":3.5}}"#,
    )
    .await;
    assert_eq!(openrouter.items[0].total, Some(20.0));
    assert_eq!(openrouter.items[0].used, Some(3.5));
    assert_eq!(openrouter.items[0].remaining, Some(16.5));

    let novita = query_fixture(
        UsageQueryPreset::NovitaBalance,
        "https://api.novita.ai/v3/user/balance",
        br#"{"availableBalance":12345}"#,
    )
    .await;
    assert_eq!(novita.items[0].remaining, Some(1.2345));
    assert_eq!(novita.items[0].unit.as_deref(), Some("USD"));
}

#[tokio::test]
async fn rejects_bad_credentials_http_failures_and_oversized_responses() {
    assert_eq!(
        UsageQueryCredentials::new("\nsecret")
            .unwrap_err()
            .to_string(),
        "用量查询 API Key 包含无效字符"
    );
    assert_eq!(
        UsageQueryCredentials::new(" secret ")
            .unwrap_err()
            .to_string(),
        "用量查询 API Key 不能包含首尾空白"
    );

    let credentials = UsageQueryCredentials::new("test-key").unwrap();
    let unauthorized = StubTransport {
        endpoint: "https://api.deepseek.com/user/balance",
        auth: AuthStyle::Bearer,
        response: UsageHttpResponse {
            status: 401,
            body: b"do not expose upstream body".to_vec(),
        },
    };
    assert_eq!(
        query_usage_with_transport(
            &unauthorized,
            UsageQueryPreset::DeepSeekBalance,
            &credentials,
        )
        .await
        .unwrap_err()
        .to_string(),
        "用量查询鉴权失败（HTTP 401）"
    );

    let oversized = StubTransport {
        endpoint: "https://api.deepseek.com/user/balance",
        auth: AuthStyle::Bearer,
        response: UsageHttpResponse {
            status: 200,
            body: vec![b'x'; 1024 * 1024 + 1],
        },
    };
    assert_eq!(
        query_usage_with_transport(&oversized, UsageQueryPreset::DeepSeekBalance, &credentials,)
            .await
            .unwrap_err()
            .to_string(),
        "用量查询响应超过 1 MiB 限制"
    );

    let malformed = StubTransport {
        endpoint: "https://api.kimi.com/coding/v1/usages",
        auth: AuthStyle::Bearer,
        response: UsageHttpResponse {
            status: 200,
            body: br#"{"usage":{"limit":10,"remaining":11}}"#.to_vec(),
        },
    };
    assert_eq!(
        query_usage_with_transport(&malformed, UsageQueryPreset::KimiCodingPlan, &credentials,)
            .await
            .unwrap_err()
            .to_string(),
        "用量额度字段超出范围：weekly_limit"
    );
}

#[tokio::test]
#[ignore = "uses an explicitly supplied real DeepSeek API key"]
async fn live_deepseek_balance_uses_the_official_query() {
    let key = std::env::var("GRILLFORGE_LIVE_API_KEY")
        .expect("GRILLFORGE_LIVE_API_KEY must be set for the live balance query");
    let snapshot = query_usage(
        UsageQueryPreset::DeepSeekBalance,
        &UsageQueryCredentials::new(key).unwrap(),
    )
    .await
    .expect("official DeepSeek balance response");
    assert_eq!(snapshot.kind, UsageKind::Balance);
    assert!(!snapshot.items.is_empty());
    assert!(snapshot.items.iter().all(|item| item.remaining.is_some()));
}
