use std::{
    fmt,
    time::{Duration, SystemTime},
};
use ureq::http::HeaderMap;

#[derive(Debug)]
pub(crate) struct RateLimited;
impl fmt::Display for RateLimited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HTTP 429 rate limit")
    }
}
impl std::error::Error for RateLimited {}

pub(crate) fn retry_after(headers: &HeaderMap) -> Duration {
    let Some(value) = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
    else {
        return Duration::from_secs(5);
    };
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        // A syntactically valid but enormous delay must not become a short retry.
        return value
            .parse::<u64>()
            .map(Duration::from_secs)
            .unwrap_or(Duration::MAX);
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|date| date.duration_since(SystemTime::now()).unwrap_or_default())
        .unwrap_or(Duration::from_secs(5))
}
