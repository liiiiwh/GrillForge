use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind {
    Balance,
    CodingPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStyle {
    Bearer,
    AuthorizationValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageQueryPreset {
    DeepSeekBalance,
    StepFunBalance,
    SiliconFlowCnBalance,
    SiliconFlowGlobalBalance,
    OpenRouterBalance,
    NovitaBalance,
    KimiCodingPlan,
    ZhipuCnCodingPlan,
    ZhipuGlobalCodingPlan,
    MiniMaxCnCodingPlan,
    MiniMaxGlobalCodingPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageQueryCapability {
    pub preset: UsageQueryPreset,
    pub label: &'static str,
    pub kind: UsageKind,
    pub endpoint: &'static str,
    pub auth: AuthStyle,
}

#[derive(Clone)]
pub struct UsageQueryCredentials {
    api_key: String,
}

impl UsageQueryCredentials {
    pub fn new(api_key: impl Into<String>) -> Result<Self, UsageQueryError> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(UsageQueryError::InvalidCredential(
                "用量查询 API Key 不能为空".to_owned(),
            ));
        }
        if api_key.contains(['\r', '\n']) {
            return Err(UsageQueryError::InvalidCredential(
                "用量查询 API Key 包含无效字符".to_owned(),
            ));
        }
        if api_key.trim() != api_key {
            return Err(UsageQueryError::InvalidCredential(
                "用量查询 API Key 不能包含首尾空白".to_owned(),
            ));
        }
        Ok(Self { api_key })
    }
}

impl fmt::Debug for UsageQueryCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("UsageQueryCredentials([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageHttpRequest {
    pub endpoint: &'static str,
    pub auth: AuthStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

pub trait UsageTransport {
    fn get<'a>(
        &'a self,
        request: &'a UsageHttpRequest,
        api_key: &'a str,
    ) -> impl Future<Output = Result<UsageHttpResponse, UsageQueryError>> + Send + 'a;
}

#[derive(Clone)]
struct ReqwestUsageTransport {
    client: reqwest::Client,
}

impl ReqwestUsageTransport {
    fn new() -> Result<Self, UsageQueryError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| transport_error("初始化 HTTP 客户端", error))?;
        Ok(Self { client })
    }
}

impl UsageTransport for ReqwestUsageTransport {
    async fn get(
        &self,
        request: &UsageHttpRequest,
        api_key: &str,
    ) -> Result<UsageHttpResponse, UsageQueryError> {
        let authorization = match request.auth {
            AuthStyle::Bearer => format!("Bearer {api_key}"),
            AuthStyle::AuthorizationValue => api_key.to_owned(),
        };
        let authorization =
            reqwest::header::HeaderValue::from_str(&authorization).map_err(|_| {
                UsageQueryError::InvalidCredential(
                    "用量查询 API Key 不能作为 HTTP Authorization 头".to_owned(),
                )
            })?;
        let mut request_builder = self
            .client
            .get(request.endpoint)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::AUTHORIZATION, authorization);
        if request.auth == AuthStyle::AuthorizationValue {
            request_builder = request_builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en");
        }
        let mut response = request_builder
            .send()
            .await
            .map_err(|error| transport_error("发送请求", error))?;
        let status = response.status().as_u16();
        if !(200..=299).contains(&status) {
            return Ok(UsageHttpResponse {
                status,
                body: Vec::new(),
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(UsageQueryError::ResponseTooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| transport_error("读取响应", error))?
        {
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(UsageQueryError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(UsageHttpResponse { status, body })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub preset: UsageQueryPreset,
    pub kind: UsageKind,
    pub queried_at_unix_ms: i64,
    pub items: Vec<UsageItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageItem {
    pub label: String,
    pub total: Option<f64>,
    pub used: Option<f64>,
    pub remaining: Option<f64>,
    pub unit: Option<String>,
    pub utilization_percent: Option<f64>,
    pub reset_at: Option<ResetAt>,
    pub valid: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ResetAt {
    UnixMilliseconds(i64),
    Iso8601(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageQueryError {
    InvalidCredential(String),
    Authentication(u16),
    HttpStatus(u16),
    ResponseTooLarge,
    InvalidResponse(String),
    Transport(String),
}

impl fmt::Display for UsageQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCredential(message)
            | Self::InvalidResponse(message)
            | Self::Transport(message) => formatter.write_str(message),
            Self::Authentication(status) => {
                write!(formatter, "用量查询鉴权失败（HTTP {status}）")
            }
            Self::HttpStatus(status) => write!(formatter, "用量查询返回 HTTP {status}"),
            Self::ResponseTooLarge => formatter.write_str("用量查询响应超过 1 MiB 限制"),
        }
    }
}

impl std::error::Error for UsageQueryError {}

const CAPABILITIES: [UsageQueryCapability; 11] = [
    UsageQueryCapability {
        preset: UsageQueryPreset::DeepSeekBalance,
        label: "DeepSeek 余额",
        kind: UsageKind::Balance,
        endpoint: "https://api.deepseek.com/user/balance",
        auth: AuthStyle::Bearer,
    },
    UsageQueryCapability {
        preset: UsageQueryPreset::StepFunBalance,
        label: "StepFun 余额",
        kind: UsageKind::Balance,
        endpoint: "https://api.stepfun.com/v1/accounts",
        auth: AuthStyle::Bearer,
    },
    UsageQueryCapability {
        preset: UsageQueryPreset::SiliconFlowCnBalance,
        label: "SiliconFlow 余额",
        kind: UsageKind::Balance,
        endpoint: "https://api.siliconflow.cn/v1/user/info",
        auth: AuthStyle::Bearer,
    },
    UsageQueryCapability {
        preset: UsageQueryPreset::SiliconFlowGlobalBalance,
        label: "SiliconFlow Global 余额",
        kind: UsageKind::Balance,
        endpoint: "https://api.siliconflow.com/v1/user/info",
        auth: AuthStyle::Bearer,
    },
    UsageQueryCapability {
        preset: UsageQueryPreset::OpenRouterBalance,
        label: "OpenRouter 余额",
        kind: UsageKind::Balance,
        endpoint: "https://openrouter.ai/api/v1/credits",
        auth: AuthStyle::Bearer,
    },
    UsageQueryCapability {
        preset: UsageQueryPreset::NovitaBalance,
        label: "Novita AI 余额",
        kind: UsageKind::Balance,
        endpoint: "https://api.novita.ai/v3/user/balance",
        auth: AuthStyle::Bearer,
    },
    UsageQueryCapability {
        preset: UsageQueryPreset::KimiCodingPlan,
        label: "Kimi For Coding 套餐",
        kind: UsageKind::CodingPlan,
        endpoint: "https://api.kimi.com/coding/v1/usages",
        auth: AuthStyle::Bearer,
    },
    UsageQueryCapability {
        preset: UsageQueryPreset::ZhipuCnCodingPlan,
        label: "智谱 GLM 套餐",
        kind: UsageKind::CodingPlan,
        endpoint: "https://open.bigmodel.cn/api/monitor/usage/quota/limit",
        auth: AuthStyle::AuthorizationValue,
    },
    UsageQueryCapability {
        preset: UsageQueryPreset::ZhipuGlobalCodingPlan,
        label: "Zhipu GLM Global 套餐",
        kind: UsageKind::CodingPlan,
        endpoint: "https://api.z.ai/api/monitor/usage/quota/limit",
        auth: AuthStyle::AuthorizationValue,
    },
    UsageQueryCapability {
        preset: UsageQueryPreset::MiniMaxCnCodingPlan,
        label: "MiniMax 套餐",
        kind: UsageKind::CodingPlan,
        endpoint: "https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains",
        auth: AuthStyle::Bearer,
    },
    UsageQueryCapability {
        preset: UsageQueryPreset::MiniMaxGlobalCodingPlan,
        label: "MiniMax Global 套餐",
        kind: UsageKind::CodingPlan,
        endpoint: "https://api.minimax.io/v1/api/openplatform/coding_plan/remains",
        auth: AuthStyle::Bearer,
    },
];

pub fn supported_usage_queries() -> &'static [UsageQueryCapability] {
    &CAPABILITIES
}

pub async fn query_usage(
    preset: UsageQueryPreset,
    credentials: &UsageQueryCredentials,
) -> Result<UsageSnapshot, UsageQueryError> {
    let transport = ReqwestUsageTransport::new()?;
    query_usage_with_transport(&transport, preset, credentials).await
}

fn capability(preset: UsageQueryPreset) -> &'static UsageQueryCapability {
    CAPABILITIES
        .iter()
        .find(|capability| capability.preset == preset)
        .expect("every preset has a vetted descriptor")
}

pub async fn query_usage_with_transport<T: UsageTransport>(
    transport: &T,
    preset: UsageQueryPreset,
    credentials: &UsageQueryCredentials,
) -> Result<UsageSnapshot, UsageQueryError> {
    let descriptor = capability(preset);
    let response = transport
        .get(
            &UsageHttpRequest {
                endpoint: descriptor.endpoint,
                auth: descriptor.auth,
            },
            &credentials.api_key,
        )
        .await?;
    if response.body.len() > MAX_RESPONSE_BYTES {
        return Err(UsageQueryError::ResponseTooLarge);
    }
    match response.status {
        200..=299 => {}
        401 | 403 => return Err(UsageQueryError::Authentication(response.status)),
        status => return Err(UsageQueryError::HttpStatus(status)),
    }
    let body: serde_json::Value = serde_json::from_slice(&response.body).map_err(|error| {
        UsageQueryError::InvalidResponse(format!("用量响应不是有效 JSON：{error}"))
    })?;
    let items = parse_items(preset, &body)?;
    if items.is_empty() {
        return Err(UsageQueryError::InvalidResponse(
            "用量响应没有可显示的数据".to_owned(),
        ));
    }
    Ok(UsageSnapshot {
        preset,
        kind: descriptor.kind,
        queried_at_unix_ms: now_unix_ms()?,
        items,
    })
}

fn parse_items(
    preset: UsageQueryPreset,
    body: &serde_json::Value,
) -> Result<Vec<UsageItem>, UsageQueryError> {
    match preset {
        UsageQueryPreset::DeepSeekBalance => parse_deepseek(body),
        UsageQueryPreset::StepFunBalance => Ok(vec![balance_item(
            "StepFun",
            number_field(body, "balance")?,
            "CNY",
        )]),
        UsageQueryPreset::SiliconFlowCnBalance => parse_siliconflow(body, "SiliconFlow", "CNY"),
        UsageQueryPreset::SiliconFlowGlobalBalance => {
            parse_siliconflow(body, "SiliconFlow Global", "USD")
        }
        UsageQueryPreset::OpenRouterBalance => parse_openrouter(body),
        UsageQueryPreset::NovitaBalance => {
            let remaining = number_field(body, "availableBalance")? / 10_000.0;
            Ok(vec![balance_item("Novita AI", remaining, "USD")])
        }
        UsageQueryPreset::KimiCodingPlan => parse_kimi(body),
        UsageQueryPreset::ZhipuCnCodingPlan | UsageQueryPreset::ZhipuGlobalCodingPlan => {
            parse_zhipu(body)
        }
        UsageQueryPreset::MiniMaxCnCodingPlan | UsageQueryPreset::MiniMaxGlobalCodingPlan => {
            parse_minimax(body)
        }
    }
}

fn plan_item(
    label: &str,
    utilization: f64,
    reset_at: Option<ResetAt>,
) -> Result<UsageItem, UsageQueryError> {
    if !(0.0..=100.0).contains(&utilization) {
        return Err(UsageQueryError::InvalidResponse(format!(
            "用量百分比超出范围：{label}"
        )));
    }
    Ok(UsageItem {
        label: label.to_owned(),
        total: Some(100.0),
        used: Some(utilization),
        remaining: Some(100.0 - utilization),
        unit: Some("%".to_owned()),
        utilization_percent: Some(utilization),
        reset_at,
        valid: Some(utilization < 100.0),
    })
}

fn parse_kimi(body: &serde_json::Value) -> Result<Vec<UsageItem>, UsageQueryError> {
    let mut items = Vec::new();
    if let Some(limits) = body.get("limits").and_then(serde_json::Value::as_array) {
        for limit in limits {
            let detail = limit
                .get("detail")
                .ok_or_else(|| invalid_field("limits[].detail"))?;
            items.push(plan_from_limit("five_hour", detail)?);
        }
    }
    if let Some(usage) = body.get("usage") {
        items.push(plan_from_limit("weekly_limit", usage)?);
    }
    Ok(items)
}

fn plan_from_limit(label: &str, value: &serde_json::Value) -> Result<UsageItem, UsageQueryError> {
    let total = number_field(value, "limit")?;
    let remaining = number_field(value, "remaining")?;
    if total <= 0.0 || remaining < 0.0 || remaining > total {
        return Err(UsageQueryError::InvalidResponse(format!(
            "用量额度字段超出范围：{label}"
        )));
    }
    let reset_at = value.get("resetTime").map(parse_reset_at).transpose()?;
    plan_item(label, (total - remaining) / total * 100.0, reset_at)
}

fn parse_zhipu(body: &serde_json::Value) -> Result<Vec<UsageItem>, UsageQueryError> {
    if body.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        return Err(UsageQueryError::InvalidResponse(
            "智谱额度接口返回业务错误".to_owned(),
        ));
    }
    let data = body.get("data").ok_or_else(|| invalid_field("data"))?;
    let limits = data
        .get("limits")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid_field("data.limits"))?;
    type Entry = (Option<i64>, f64, Option<ResetAt>);
    let mut five_hour: Option<Entry> = None;
    let mut weekly: Option<Entry> = None;
    let mut unclassified: Vec<Entry> = Vec::new();
    for limit in limits {
        let limit_type = limit
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !limit_type.eq_ignore_ascii_case("TOKENS_LIMIT") {
            continue;
        }
        let utilization = number_field(limit, "percentage")?;
        let raw_reset_at = limit
            .get("nextResetTime")
            .and_then(serde_json::Value::as_i64);
        let reset_at = limit.get("nextResetTime").map(parse_reset_at).transpose()?;
        let entry = (raw_reset_at, utilization, reset_at);
        match limit.get("unit").and_then(serde_json::Value::as_i64) {
            Some(3) if five_hour.is_none() => five_hour = Some(entry),
            Some(6) if weekly.is_none() => weekly = Some(entry),
            _ => unclassified.push(entry),
        }
    }
    unclassified.sort_by_key(|(reset_at, _, _)| (reset_at.is_some(), reset_at.unwrap_or(i64::MIN)));
    for entry in unclassified {
        if five_hour.is_none() {
            five_hour = Some(entry);
        } else if weekly.is_none() {
            weekly = Some(entry);
        }
    }
    let mut items = Vec::new();
    if let Some((_, utilization, reset_at)) = five_hour {
        items.push(plan_item("five_hour", utilization, reset_at)?);
    }
    if let Some((_, utilization, reset_at)) = weekly {
        items.push(plan_item("weekly_limit", utilization, reset_at)?);
    }
    Ok(items)
}

fn parse_minimax(body: &serde_json::Value) -> Result<Vec<UsageItem>, UsageQueryError> {
    if let Some(base_response) = body.get("base_resp") {
        let status = base_response
            .get("status_code")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| invalid_field("base_resp.status_code"))?;
        if status != 0 {
            return Err(UsageQueryError::InvalidResponse(format!(
                "MiniMax 额度接口返回业务错误码 {status}"
            )));
        }
    }
    let models = body
        .get("model_remains")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid_field("model_remains"))?;
    let general = models
        .iter()
        .find(|model| {
            model.get("model_name").and_then(serde_json::Value::as_str) == Some("general")
        })
        .ok_or_else(|| invalid_field("model_remains[general]"))?;
    let interval_remaining = number_field(general, "current_interval_remaining_percent")?;
    let interval_reset = general.get("end_time").map(parse_reset_at).transpose()?;
    let mut items = vec![plan_item(
        "five_hour",
        100.0 - interval_remaining,
        interval_reset,
    )?];
    if general
        .get("current_weekly_status")
        .and_then(serde_json::Value::as_i64)
        == Some(1)
    {
        let weekly_remaining = number_field(general, "current_weekly_remaining_percent")?;
        let weekly_reset = general
            .get("weekly_end_time")
            .map(parse_reset_at)
            .transpose()?;
        items.push(plan_item(
            "weekly_limit",
            100.0 - weekly_remaining,
            weekly_reset,
        )?);
    }
    Ok(items)
}

fn parse_reset_at(value: &serde_json::Value) -> Result<ResetAt, UsageQueryError> {
    if let Some(value) = value.as_str().filter(|value| !value.trim().is_empty()) {
        return Ok(ResetAt::Iso8601(value.to_owned()));
    }
    let timestamp = value
        .as_i64()
        .filter(|timestamp| *timestamp > 0)
        .ok_or_else(|| invalid_field("reset time"))?;
    let milliseconds = if timestamp < 1_000_000_000_000 {
        timestamp
            .checked_mul(1_000)
            .ok_or_else(|| invalid_field("reset time"))?
    } else {
        timestamp
    };
    Ok(ResetAt::UnixMilliseconds(milliseconds))
}

fn balance_item(label: &str, remaining: f64, unit: &str) -> UsageItem {
    UsageItem {
        label: label.to_owned(),
        total: None,
        used: None,
        remaining: Some(remaining),
        unit: Some(unit.to_owned()),
        utilization_percent: None,
        reset_at: None,
        valid: Some(remaining > 0.0),
    }
}

fn parse_siliconflow(
    body: &serde_json::Value,
    label: &str,
    unit: &str,
) -> Result<Vec<UsageItem>, UsageQueryError> {
    let data = body.get("data").ok_or_else(|| invalid_field("data"))?;
    Ok(vec![balance_item(
        label,
        number_field(data, "totalBalance")?,
        unit,
    )])
}

fn parse_openrouter(body: &serde_json::Value) -> Result<Vec<UsageItem>, UsageQueryError> {
    let data = body.get("data").unwrap_or(body);
    let total = number_field(data, "total_credits")?;
    let used = number_field(data, "total_usage")?;
    let remaining = total - used;
    Ok(vec![UsageItem {
        label: "OpenRouter".to_owned(),
        total: Some(total),
        used: Some(used),
        remaining: Some(remaining),
        unit: Some("USD".to_owned()),
        utilization_percent: (total > 0.0).then_some(used / total * 100.0),
        reset_at: None,
        valid: Some(remaining > 0.0),
    }])
}

fn parse_deepseek(body: &serde_json::Value) -> Result<Vec<UsageItem>, UsageQueryError> {
    let available = body
        .get("is_available")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| invalid_field("is_available"))?;
    let balances = body
        .get("balance_infos")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid_field("balance_infos"))?;
    balances
        .iter()
        .map(|balance| {
            let currency = balance
                .get("currency")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| invalid_field("balance_infos[].currency"))?;
            let remaining = number_field(balance, "total_balance")?;
            Ok(UsageItem {
                label: currency.to_owned(),
                total: None,
                used: None,
                remaining: Some(remaining),
                unit: Some(currency.to_owned()),
                utilization_percent: None,
                reset_at: None,
                valid: Some(available),
            })
        })
        .collect()
}

fn number_field(value: &serde_json::Value, field: &str) -> Result<f64, UsageQueryError> {
    let value = value.get(field).ok_or_else(|| invalid_field(field))?;
    let number = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()));
    match number {
        Some(number) if number.is_finite() => Ok(number),
        _ => Err(invalid_field(field)),
    }
}

fn invalid_field(field: &str) -> UsageQueryError {
    UsageQueryError::InvalidResponse(format!("用量响应字段无效：{field}"))
}

fn transport_error(operation: &str, error: reqwest::Error) -> UsageQueryError {
    UsageQueryError::Transport(format!("用量查询{operation}失败：{}", error.without_url()))
}

fn now_unix_ms() -> Result<i64, UsageQueryError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| UsageQueryError::InvalidResponse("系统时间早于 Unix epoch".to_owned()))?
        .as_millis();
    i64::try_from(millis)
        .map_err(|_| UsageQueryError::InvalidResponse("系统时间超出范围".to_owned()))
}
