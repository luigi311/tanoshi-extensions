use anyhow::{Result, anyhow};
use bytes::Bytes;
use log::debug;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use ureq::ResponseExt;

use crate::{
    image::{
        ImageWrapperPolicy, build_image_get_with_referer, bytes_fetch_impl, parse_image_response,
    },
    init_plugin_logging,
    operation::Operation,
    ratelimit::RateLimiter,
};

pub(crate) type Agent = ureq::Agent;
pub(crate) type HttpResponse = ureq::http::Response<ureq::Body>;

/// A fetched text document and its final URL after redirects.
/// Resolve relative links against `final_url`, not the original request URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedDocument {
    pub final_url: String,
    pub body: String,
}

#[derive(Clone)]
pub struct RateLimitedAgent {
    inner: Agent,
    limiter: Option<Arc<RateLimiter>>,
    configuration_error: Option<&'static str>,
}

impl RateLimitedAgent {
    fn new(inner: Agent, requests_per_second: Option<f64>) -> Self {
        init_plugin_logging();
        debug!(
            "Net RateLimitedAgent setup with {:?} RPS",
            requests_per_second
        );
        let (limiter, configuration_error) =
            match requests_per_second.map(RateLimiter::new).transpose() {
                Ok(limiter) => (limiter.map(Arc::new), None),
                Err(error) => (None, Some(error)),
            };
        Self {
            inner,
            limiter,
            configuration_error,
        }
    }

    fn execute(
        &self,
        url: &str,
        operation: &Operation,
        request: impl Fn() -> ureq::RequestBuilder<ureq::typestate::WithoutBody>,
    ) -> Result<HttpResponse> {
        if let Some(error) = self.configuration_error {
            log::error!("RateLimitedAgent: {error}");
            return Err(anyhow!(error));
        }
        loop {
            operation.before_attempt()?;
            debug!("GET {}", url);
            let response = request()
                .config()
                .http_status_as_error(false)
                .timeout_global(Some(operation.request_timeout()?))
                .build()
                .call()?;
            let status = response.status().as_u16();
            if status == 429 {
                let delay = crate::backoff::retry_after(response.headers());
                drop(response);
                operation.backoff(delay)?;
                continue;
            }
            if status >= 400 {
                return Err(ureq::Error::StatusCode(status).into());
            }
            return Ok(response);
        }
    }

    /// Perform a throttled GET and read the response body as text.
    pub fn fetch_text(&self, url: &str) -> Result<String> {
        self.fetch_document(url).map(|document| document.body)
    }

    /// Perform a throttled GET, retaining the final response URL and text body.
    pub fn fetch_document(&self, url: &str) -> Result<FetchedDocument> {
        let operation = Operation::new(self.limiter.clone());
        let mut response = self.execute(url, &operation, || self.inner.get(url))?;
        let final_url = response.get_uri().to_string();
        let body = response.body_mut().read_to_string()?;
        operation.remaining()?;
        Ok(FetchedDocument { final_url, body })
    }

    /// Perform a throttled GET and decode a JSON response with ureq's body limits.
    pub fn fetch_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let operation = Operation::new(self.limiter.clone());
        let mut response = self.execute(url, &operation, || self.inner.get(url))?;
        let body = response.body_mut().read_json()?;
        operation.remaining()?;
        Ok(body)
    }

    pub fn fetch_bytes(&self, url: &str, referer: Option<&str>) -> Result<Bytes> {
        self.fetch_bytes_with_wrapper_policy(url, referer, ImageWrapperPolicy::Reject)
    }

    /// Fetch an image with explicit source-specific HTML wrapper handling.
    pub fn fetch_bytes_with_wrapper_policy(
        &self,
        url: &str,
        referer: Option<&str>,
        wrappers: ImageWrapperPolicy,
    ) -> Result<Bytes> {
        let operation = Operation::new(self.limiter.clone());
        let mut getter = |url: &str| {
            let response = self.execute(url, &operation, || {
                build_image_get_with_referer(url, self.inner.get(url), referer)
            })?;
            parse_image_response(response, url, wrappers)
        };
        let bytes = bytes_fetch_impl(&mut getter, url)?;
        operation.remaining()?;
        Ok(bytes)
    }
}

pub(crate) fn build_ureq_agent(user_agent: Option<&str>) -> Agent {
    let mut cfg = Agent::config_builder().max_redirects(5);
    if let Some(ua) = user_agent
        && !ua.is_empty()
    {
        cfg = cfg.user_agent(ua);
    }
    cfg.build().into()
}

/// Agent for FlareClient's internal requests. Non-2xx statuses are returned
/// as responses instead of errors so challenge classification can read the
/// body of 403/503 pages; callers must check the status themselves. Plain
/// `RateLimitedAgent` users keep ureq's status-as-error default via
/// `build_ureq_agent` so HTTP failures surface as errors, not parseable
/// bodies.
pub(crate) fn build_lenient_ureq_agent(user_agent: Option<&str>) -> Agent {
    let mut cfg = Agent::config_builder()
        .max_redirects(5)
        .http_status_as_error(false);
    if let Some(ua) = user_agent
        && !ua.is_empty()
    {
        cfg = cfg.user_agent(ua);
    }
    cfg.build().into()
}

pub fn build_rate_limited_ureq_agent(
    user_agent: Option<&str>,
    requests_per_second: Option<f64>,
) -> RateLimitedAgent {
    let agent = build_ureq_agent(user_agent);
    RateLimitedAgent::new(agent, requests_per_second)
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn test_rate_limited_agent_creation() {
        let agent = build_rate_limited_ureq_agent(Some("TestUA"), Some(5.0));
        // Should have a limiter
        assert!(agent.limiter.is_some());
    }
    #[test]
    fn test_rate_limited_agent_no_limit() {
        let agent = build_rate_limited_ureq_agent(None, None);
        assert!(agent.limiter.is_none());
    }
}
