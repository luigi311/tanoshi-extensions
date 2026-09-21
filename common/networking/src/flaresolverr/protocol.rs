use super::cf_challenge_marker;
use crate::{FetchedDocument, operation::Operation};
use anyhow::{Context, Result, anyhow};
use serde_json::{Value as JsonValue, json};
use std::error::Error;

#[allow(non_snake_case)]
#[derive(Debug, serde::Deserialize, Clone)]
pub(super) struct FlareSolverrSolution {
    pub url: String,
    pub status: u16,
    pub cookies: Vec<FlareSolverrCookie>,
    pub userAgent: String,
    #[serde(rename = "headers")]
    _headers: JsonValue,
    pub response: String,
}

#[allow(non_snake_case)]
#[derive(Debug, serde::Deserialize, Clone)]
pub(super) struct FlareSolverrCookie {
    pub domain: String,
    pub expiry: Option<u64>,
    pub httpOnly: bool,
    pub name: String,
    pub path: String,
    pub sameSite: String,
    pub secure: bool,
    pub value: String,
}

#[derive(Debug)]
struct MissingFlareSolverrSession(String);

impl std::fmt::Display for MissingFlareSolverrSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FlareSolverr error: {}", self.0)
    }
}

impl Error for MissingFlareSolverrSession {}

pub(super) fn is_missing_flaresolverr_session(error: &anyhow::Error) -> bool {
    super::solve::error_is::<MissingFlareSolverrSession>(error)
}

/// Read the RPC envelope even on HTTP errors: FlareSolverr sends useful error
/// messages with HTTP 500 and without a solution. Interpret messages only here.
pub(super) fn flaresolverr_rpc(
    fs_url: &str,
    payload: &JsonValue,
    operation: &Operation,
) -> Result<JsonValue> {
    let command = payload["cmd"].as_str().unwrap_or("unknown command");
    let mut response = ureq::post(fs_url)
        .config()
        .http_status_as_error(false)
        .timeout_global(Some(operation.request_timeout()?))
        .build()
        .header("Content-Type", "application/json")
        .send_json(payload)
        .with_context(|| format!("FlareSolverr {command} RPC failed"))?;
    let status = response.status();
    if status.as_u16() == 429 {
        return Err(anyhow::Error::new(crate::backoff::RateLimited)
            .context(format!("FlareSolverr {command} RPC returned HTTP 429")));
    }
    let text = response.body_mut().read_to_string().with_context(|| {
        format!("FlareSolverr {command} HTTP {status}: could not read response")
    })?;
    let envelope: JsonValue = serde_json::from_str(&text)
        .with_context(|| format!("FlareSolverr {command} HTTP {status}: invalid JSON response"))?;
    match envelope["status"].as_str() {
        Some("ok") => {
            if !status.is_success() {
                return Err(anyhow!("FlareSolverr {command} RPC returned HTTP {status}"));
            }
            Ok(envelope)
        }
        Some("error") => {
            let message = envelope["message"]
                .as_str()
                .unwrap_or("no error message supplied");
            let lower = message.to_ascii_lowercase();
            let error = if lower.contains("session")
                && (lower.contains("not found")
                    || lower.contains("does not exist")
                    || lower.contains("doesn't exist")
                    || lower.contains("invalid"))
            {
                anyhow::Error::new(MissingFlareSolverrSession(message.to_string()))
            } else {
                anyhow!("FlareSolverr error: {message}")
            };
            Err(error.context(format!(
                "FlareSolverr {command} RPC HTTP {status}: {message}"
            )))
        }
        _ => Err(anyhow!(
            "FlareSolverr {command} HTTP {status}: missing or unknown envelope status"
        )),
    }
}

/// Validate reported upstream failures and residual challenges. FlareSolverr
/// 3.5.2 synthesizes status 200, so this cannot detect unreported HTTP failures;
/// source parsers must still validate the returned content.
pub(super) fn request_flaresolverr(
    fs_url: &str,
    payload: &JsonValue,
    operation: &Operation,
) -> Result<FlareSolverrSolution> {
    // Solves and ordinary GET/POST proxy requests navigate the same browser.
    // Stateless requests have their own browser and need no shared lock.
    let browser = payload["session"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(|id| super::session::browser_lock(fs_url, id, operation))
        .transpose()?;
    let _navigation = browser
        .as_ref()
        .map(|lock| operation.lock(lock))
        .transpose()?;
    operation.before_attempt()?;
    let mut payload = payload.clone();
    payload["maxTimeout"] = json!(operation.request_timeout()?.as_millis().max(1) as u64);
    let mut envelope = flaresolverr_rpc(fs_url, &payload, operation)?;
    let solution: FlareSolverrSolution = serde_json::from_value(envelope["solution"].take())
        .context("FlareSolverr returned an invalid or missing solution")?;
    if solution.status == 429 {
        return Err(
            anyhow::Error::new(crate::backoff::RateLimited).context(format!(
                "FlareSolverr reported upstream HTTP 429 for {}",
                solution.url
            )),
        );
    }
    if cf_challenge_marker(solution.status, &solution.response).is_some() {
        return Err(anyhow!(
            "FlareSolverr returned an unsolved challenge (HTTP {}) for {}",
            solution.status,
            solution.url
        ));
    }
    // Match the direct path's status policy after redirects.
    if solution.status >= 400 {
        return Err(anyhow!(
            "FlareSolverr returned upstream HTTP {} for {}",
            solution.status,
            solution.url
        ));
    }
    Ok(solution)
}

pub(super) fn proxy_fetch_document(
    fs_url: &str,
    session_id: Option<&str>,
    url: &str,
    operation: &Operation,
) -> Result<FetchedDocument> {
    let payload = match session_id {
        Some(sid) => json!({"cmd":"request.get","url":url,"session":sid}),
        None => json!({"cmd":"request.get","url":url}),
    };

    let solution = request_flaresolverr(fs_url, &payload, operation)?;
    Ok(FetchedDocument {
        final_url: solution.url,
        body: solution.response,
    })
}

pub(super) fn proxy_post_form(
    fs_url: &str,
    session_id: Option<&str>,
    url: &str,
    form: &[(&str, &str)],
    operation: &Operation,
) -> Result<FetchedDocument> {
    let body = form
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let payload = match session_id {
        Some(sid) => json!({
            "cmd": "request.post",
            "url": url,
            "session": sid,
            "postData": body,
        }),
        None => json!({
            "cmd": "request.post",
            "url": url,
            "postData": body,
        }),
    };

    let solution = request_flaresolverr(fs_url, &payload, operation)?;
    Ok(FetchedDocument {
        final_url: solution.url,
        body: solution.response,
    })
}

pub(super) fn proxy_post_empty(
    fs_url: &str,
    session_id: Option<&str>,
    url: &str,
    operation: &Operation,
) -> Result<FetchedDocument> {
    let payload = match session_id {
        Some(sid) => json!({
            "cmd": "request.post",
            "url": url,
            "session": sid,
            "postData": "",
        }),
        None => json!({
            "cmd": "request.post",
            "url": url,
            "postData": "",
        }),
    };

    let solution = request_flaresolverr(fs_url, &payload, operation)?;
    Ok(FetchedDocument {
        final_url: solution.url,
        body: solution.response,
    })
}

pub(super) struct Solved {
    pub(super) url: String,
    pub(super) user_agent: String,
    pub(super) cookies: Vec<FlareSolverrCookie>,
}

pub(super) fn solve_with_flaresolverr(
    flaresolverr_url: &str,
    url: &str,
    session: Option<&str>,
    operation: &Operation,
) -> Result<Solved> {
    let payload = match session {
        Some(sid) => json!({"cmd":"request.get","url":url,"session":sid}),
        None => json!({"cmd":"request.get","url":url}),
    };

    let solution = request_flaresolverr(flaresolverr_url, &payload, operation)?;

    // Solution headers describe the browser response, not future requests.
    Ok(Solved {
        url: solution.url,
        user_agent: solution.userAgent,
        cookies: solution.cookies,
    })
}
