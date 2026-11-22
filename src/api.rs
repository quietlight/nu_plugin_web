//! The two HTTP calls this plugin can make, the request bodies it builds and
//! the replies it shapes for nu.
//!
//! The constraints are the point of the plugin, so they live here as
//! constants rather than flags: one fixed host, two fixed paths, capped
//! request size, capped result count and excerpt budget, and a timeout.

use std::time::Duration;

use nu_protocol::{LabeledError, Record, Span, Value};
use serde_json::{Map, Value as Json, json};

/// The only host the plugin talks to. Not configurable on purpose.
const HOST: &str = "https://api.parallel.ai";
const SEARCH_PATH: &str = "/v1/search";
const EXTRACT_PATH: &str = "/v1/extract";

/// Search modes the API accepts. `fast` is the default: $1 per 1k requests
/// against $5 for basic/advanced.
pub const MODES: [&str; 4] = ["turbo", "fast", "basic", "advanced"];
pub const DEFAULT_MODE: &str = "fast";

/// Results past 10 are billed extra, so 10 is the ceiling.
pub const MAX_RESULTS: i64 = 10;
pub const DEFAULT_RESULTS: i64 = 5;

/// Total characters of excerpt (or full content) that come back. The default
/// keeps a reply to a few thousand tokens of the agent's context.
pub const DEFAULT_CHARS: i64 = 20_000;
pub const MAX_CHARS: i64 = 100_000;

/// The Extract API takes at most 20 URLs per request.
pub const MAX_URLS: usize = 20;

/// Upper bound on the serialized request body. Queries and URLs are small;
/// the cap exists so a runaway pipeline cannot ship a large file off the
/// machine inside an objective.
const MAX_BODY_BYTES: usize = 16 * 1024;

const TIMEOUT: Duration = Duration::from_secs(60);

/// Where the key is read from, relative to `$HOME`. A file rather than an
/// environment variable, so an agent's nu script never sees the key in
/// `$env`.
const KEY_FILE: &str = ".config/parallel/api_key";

/// Fallback for interactive use when there is no key file.
pub const KEY_ENV: &str = "PARALLEL_API_KEY";

pub fn err(msg: impl Into<String>, span: Span) -> LabeledError {
    let msg = msg.into();
    LabeledError::new(msg.clone()).with_label(msg, span)
}

/// The API key: `~/.config/parallel/api_key`, else `$env.PARALLEL_API_KEY`
/// as nu sees it.
pub fn api_key(env_value: Option<Value>, span: Span) -> Result<String, LabeledError> {
    if let Some(home) = std::env::var_os("HOME") {
        let path = std::path::Path::new(&home).join(KEY_FILE);
        if let Ok(text) = std::fs::read_to_string(&path) {
            let key = text.trim();
            if !key.is_empty() {
                return Ok(key.to_string());
            }
        }
    }
    match env_value {
        Some(Value::String { val, .. }) if !val.trim().is_empty() => Ok(val.trim().to_string()),
        _ => Err(err("no Parallel API key", span).with_help(format!(
            "put the key in ~/{KEY_FILE} (preferred) or set ${KEY_ENV}"
        ))),
    }
}

/// `n` clamped to `1..=max`, with an error naming the flag when it is out of
/// range rather than a silent clamp.
pub fn bounded(flag: &str, n: i64, max: i64, span: Span) -> Result<i64, LabeledError> {
    if (1..=max).contains(&n) {
        Ok(n)
    } else {
        Err(err(format!("--{flag} must be 1 to {max}, got {n}"), span))
    }
}

pub struct Search {
    pub objective: String,
    pub queries: Vec<String>,
    pub mode: String,
    pub max_results: i64,
    pub max_chars: i64,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub after: Option<String>,
}

/// The `/v1/search` body. With no queries the objective doubles as the one
/// query and is not also sent as the objective (the docs advise against
/// sending the same text twice).
pub fn search_body(s: &Search) -> Json {
    let mut body = Map::new();
    if s.queries.is_empty() {
        body.insert("search_queries".into(), json!([s.objective]));
    } else {
        body.insert("objective".into(), json!(s.objective));
        body.insert("search_queries".into(), json!(s.queries));
    }
    body.insert("mode".into(), json!(s.mode));
    body.insert("max_chars_total".into(), json!(s.max_chars));

    let mut advanced = Map::new();
    advanced.insert("max_results".into(), json!(s.max_results));
    let mut policy = Map::new();
    if !s.include.is_empty() {
        policy.insert("include_domains".into(), json!(s.include));
    }
    if !s.exclude.is_empty() {
        policy.insert("exclude_domains".into(), json!(s.exclude));
    }
    if let Some(after) = &s.after {
        policy.insert("after_date".into(), json!(after));
    }
    if !policy.is_empty() {
        advanced.insert("source_policy".into(), Json::Object(policy));
    }
    body.insert("advanced_settings".into(), Json::Object(advanced));
    Json::Object(body)
}

pub struct Extract {
    pub urls: Vec<String>,
    pub objective: Option<String>,
    pub full: bool,
    pub max_chars: i64,
}

/// The `/v1/extract` body. `--full` asks for each page's whole text, capped
/// per page so the total stays within `max_chars`.
pub fn extract_body(e: &Extract) -> Json {
    let mut body = Map::new();
    body.insert("urls".into(), json!(e.urls));
    if let Some(objective) = &e.objective {
        body.insert("objective".into(), json!(objective));
    }
    body.insert("max_chars_total".into(), json!(e.max_chars));
    if e.full {
        let per_page = (e.max_chars / e.urls.len().max(1) as i64).max(1);
        body.insert(
            "advanced_settings".into(),
            json!({ "full_content": { "max_chars_per_result": per_page } }),
        );
    }
    Json::Object(body)
}

pub fn search(key: &str, body: &Json, span: Span) -> Result<Json, LabeledError> {
    post(key, SEARCH_PATH, body, span)
}

pub fn extract(key: &str, body: &Json, span: Span) -> Result<Json, LabeledError> {
    post(key, EXTRACT_PATH, body, span)
}

fn post(key: &str, path: &str, body: &Json, span: Span) -> Result<Json, LabeledError> {
    let bytes = serde_json::to_vec(body).map_err(|e| err(e.to_string(), span))?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(err(
            format!(
                "request is {} KiB, over the {} KiB cap",
                bytes.len() / 1024,
                MAX_BODY_BYTES / 1024
            ),
            span,
        )
        .with_help("objectives and queries should be a sentence or a few words"));
    }

    // Status codes come back as responses, not errors, so a 422 can surface
    // the API's own explanation.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .post(format!("{HOST}{path}"))
        .header("x-api-key", key)
        .header("Content-Type", "application/json")
        .send(&bytes[..])
        .map_err(|e| err(format!("request to Parallel failed: {e}"), span))?;

    let status = response.status();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| err(format!("reading Parallel's reply failed: {e}"), span))?;
    if !status.is_success() {
        return Err(
            err(format!("Parallel returned {status}"), span).with_help(error_message(&text))
        );
    }
    serde_json::from_str(&text).map_err(|e| err(format!("Parallel's reply is not JSON: {e}"), span))
}

/// `error.message` (and `detail`) from an error body, else the raw text.
fn error_message(text: &str) -> String {
    let Ok(json) = serde_json::from_str::<Json>(text) else {
        return text.to_string();
    };
    let e = json.get("error");
    let message = e.and_then(|e| e.get("message")).and_then(Json::as_str);
    let detail = e.and_then(|e| e.get("detail")).filter(|d| !d.is_null());
    match (message, detail) {
        (Some(m), Some(d)) => format!("{m}: {d}"),
        (Some(m), None) => m.to_string(),
        _ => text.to_string(),
    }
}

fn field(item: &Json, key: &str, span: Span) -> Value {
    item.get(key)
        .map_or_else(|| Value::nothing(span), |j| from_json(j, span))
}

fn results(reply: &Json) -> &[Json] {
    reply
        .get("results")
        .and_then(Json::as_array)
        .map_or(&[], Vec::as_slice)
}

/// A search reply as a table of `{url, title, publish_date, excerpts}`.
pub fn search_table(reply: &Json, span: Span) -> Value {
    let rows = results(reply)
        .iter()
        .map(|item| {
            let mut row = Record::new();
            for key in ["url", "title", "publish_date", "excerpts"] {
                row.push(key, field(item, key, span));
            }
            Value::record(row, span)
        })
        .collect();
    Value::list(rows, span)
}

/// An extract reply as one row per URL: `{url, title, publish_date,
/// excerpts, full_content?}` for pages that came back, `{url, error}` for
/// pages that did not.
pub fn extract_table(reply: &Json, full: bool, span: Span) -> Value {
    let mut rows: Vec<Value> = results(reply)
        .iter()
        .map(|item| {
            let mut row = Record::new();
            for key in ["url", "title", "publish_date", "excerpts"] {
                row.push(key, field(item, key, span));
            }
            if full {
                row.push("full_content", field(item, "full_content", span));
            }
            Value::record(row, span)
        })
        .collect();
    let errors = reply
        .get("errors")
        .and_then(Json::as_array)
        .map_or(&[][..], Vec::as_slice);
    for e in errors {
        let kind = e
            .get("error_type")
            .and_then(Json::as_str)
            .unwrap_or("error");
        let status = e
            .get("http_status_code")
            .and_then(Json::as_i64)
            .map_or(String::new(), |c| format!(" ({c})"));
        let content = e
            .get("content")
            .and_then(Json::as_str)
            .map_or(String::new(), |c| format!(": {c}"));
        let mut row = Record::new();
        row.push("url", field(e, "url", span));
        row.push(
            "error",
            Value::string(format!("{kind}{status}{content}"), span),
        );
        rows.push(Value::record(row, span));
    }
    Value::list(rows, span)
}

/// JSON -> nu value, for `--raw` replies.
pub fn from_json(json: &Json, span: Span) -> Value {
    match json {
        Json::Null => Value::nothing(span),
        Json::Bool(b) => Value::bool(*b, span),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::int(i, span),
            None => Value::float(n.as_f64().unwrap_or(f64::NAN), span),
        },
        Json::String(s) => Value::string(s.clone(), span),
        Json::Array(items) => Value::list(items.iter().map(|j| from_json(j, span)).collect(), span),
        Json::Object(map) => {
            let mut record = Record::new();
            for (k, v) in map {
                record.push(k.clone(), from_json(v, span));
            }
            Value::record(record, span)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(queries: &[&str]) -> Search {
        Search {
            objective: "kauri dieback research".into(),
            queries: queries.iter().map(|q| q.to_string()).collect(),
            mode: DEFAULT_MODE.into(),
            max_results: DEFAULT_RESULTS,
            max_chars: DEFAULT_CHARS,
            include: vec![],
            exclude: vec![],
            after: None,
        }
    }

    #[test]
    fn objective_alone_becomes_the_query() {
        let body = search_body(&search(&[]));
        assert_eq!(body["search_queries"], json!(["kauri dieback research"]));
        assert!(body.get("objective").is_none());
        assert_eq!(body["mode"], "fast");
        assert_eq!(body["advanced_settings"], json!({ "max_results": 5 }));
    }

    #[test]
    fn queries_and_source_policy_go_where_v1_wants_them() {
        let mut s = search(&["kauri dieback 2026"]);
        s.include = vec!["govt.nz".into()];
        s.after = Some("2025-01-01".into());
        let body = search_body(&s);
        assert_eq!(body["objective"], "kauri dieback research");
        assert_eq!(body["search_queries"], json!(["kauri dieback 2026"]));
        assert_eq!(body["max_chars_total"], 20_000);
        // v1 rejects these at the top level (422).
        assert!(body.get("max_results").is_none());
        assert!(body.get("source_policy").is_none());
        assert_eq!(
            body["advanced_settings"]["source_policy"],
            json!({ "include_domains": ["govt.nz"], "after_date": "2025-01-01" })
        );
    }

    #[test]
    fn full_content_is_capped_per_page() {
        let body = extract_body(&Extract {
            urls: vec!["https://a.example".into(), "https://b.example".into()],
            objective: None,
            full: true,
            max_chars: 20_000,
        });
        assert_eq!(
            body["advanced_settings"]["full_content"]["max_chars_per_result"],
            10_000
        );
        assert!(body.get("objective").is_none());
    }

    #[test]
    fn search_reply_becomes_a_table() {
        let reply: Json = serde_json::from_str(include_str!("../tests/data/search.json")).unwrap();
        let table = search_table(&reply, Span::test_data());
        let rows = table.as_list().unwrap();
        assert_eq!(rows.len(), 2);
        let first = rows[0].as_record().unwrap();
        assert_eq!(
            first.columns().cloned().collect::<Vec<_>>(),
            ["url", "title", "publish_date", "excerpts"]
        );
        assert_eq!(
            first.get("url").unwrap().as_str().unwrap(),
            "https://www.doc.govt.nz/nature/pests-and-threats/diseases/kauri-disease"
        );
        assert!(
            rows[1]
                .as_record()
                .unwrap()
                .get("publish_date")
                .unwrap()
                .is_nothing()
        );
    }

    #[test]
    fn extract_errors_become_error_rows() {
        let reply = json!({
            "extract_id": "extract_1",
            "results": [{ "url": "https://ok.example", "title": "OK", "excerpts": ["hi"] }],
            "errors": [{ "url": "https://bad.example", "error_type": "fetch_error",
                         "http_status_code": 500, "content": "Error fetching" }],
            "session_id": "session_1"
        });
        let table = extract_table(&reply, false, Span::test_data());
        let rows = table.as_list().unwrap();
        assert_eq!(rows.len(), 2);
        let bad = rows[1].as_record().unwrap();
        assert_eq!(
            bad.get("error").unwrap().as_str().unwrap(),
            "fetch_error (500): Error fetching"
        );
    }

    #[test]
    fn error_body_message_is_surfaced() {
        let text =
            r#"{"type":"error","error":{"ref_id":"r1","message":"Invalid request","detail":null}}"#;
        assert_eq!(error_message(text), "Invalid request");
        assert_eq!(error_message("gateway down"), "gateway down");
    }
}
