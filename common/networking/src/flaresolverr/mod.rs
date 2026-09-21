mod config;
pub use config::{FlareClientConfig, FlareSession};
mod cookies;
mod json;
pub use json::parse_browser_json;
mod protocol;
mod session;
mod solve;
use session::{SessionState, is_managed_session_error};
use solve::SolveAttempt;

use crate::{
    client::{Agent, FetchedDocument, HttpResponse, build_lenient_ureq_agent},
    image::{
        ImageChallenge, ImageResponse, ImageWrapperPolicy, build_image_get_with_referer,
        bytes_fetch_impl, parse_image_response,
    },
    init_plugin_logging,
    operation::Operation,
    ratelimit::RateLimiter,
};
use anyhow::{Context, Result, anyhow};
use bytes::Bytes;
use cookies::insert_flaresolverr_cookies_into_agent;
use log::{debug, info, warn};
use protocol::{
    is_missing_flaresolverr_session, proxy_fetch_document, proxy_post_empty, proxy_post_form,
    solve_with_flaresolverr,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use ureq::ResponseExt;

const DIRECT_RETRY_COOLDOWN: Duration = Duration::from_secs(15 * 60);
/// User agent for FlareClient agents before the first FlareSolverr solve
/// replaces them with the solved browser's UA. Sites that block default
/// library user agents outright would otherwise 403 every first contact.
const DEFAULT_BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:132.0) Gecko/20100101 Firefox/132.0";

pub fn build_rate_limited_flaresolverr_client(
    origin_url: &str,
    requests_per_second: Option<f64>,
) -> FlareClient {
    FlareClient::new(FlareClientConfig::from_env(
        origin_url,
        requests_per_second,
        None,
    ))
}

/// Build a FlareClient with a persistent, extension-scoped FlareSolverr
/// session. The session is discovered/created lazily on the first request
/// that needs FlareSolverr, then reused by subsequent requests.
pub fn build_rate_limited_flaresolverr_client_for_extension(
    origin_url: &str,
    requests_per_second: Option<f64>,
    extension_name: &str,
) -> FlareClient {
    let session_name = format!("tanoshi-{extension_name}");
    FlareClient::new(FlareClientConfig::from_env(
        origin_url,
        requests_per_second,
        Some(&session_name),
    ))
}

/// Internal, mutable state wrapped by a Mutex.
#[derive(Clone)]
struct Inner {
    agent: Agent,
    origin_url: String,
    flaresolverr_url: Option<String>,
    session: SessionState,
    default_headers: Vec<(String, String)>,
    limiter: Option<Arc<RateLimiter>>,
    route: RouteState,
}

#[derive(Clone, Copy)]
enum RouteState {
    DirectEligible,
    ProxyUntil(Instant),
}

/// Public handle that is Send + Sync.
#[derive(Clone)]
pub struct FlareClient {
    solve_attempt: Arc<Mutex<Arc<SolveAttempt>>>,
    inner: Arc<Mutex<Inner>>,
    configuration_error: Option<&'static str>,
}

/// Heuristic: does this response body look like a Cloudflare challenge page?
/// Returns the marker that matched so routing decisions can be traced in logs.
pub(crate) fn cf_challenge_marker(status: u16, body: &str) -> Option<&'static str> {
    let lower = body.to_ascii_lowercase();

    // Cloudflare challenge pages contain characteristic markers. The generic
    // challenge-platform script also runs passive detection on normal pages,
    // so its presence alone must not mark a response as an unsolved challenge.
    // We require at least one challenge-specific marker AND the word "cloudflare"
    // in the body, even for 403/503 status codes. A bare 403 without CF markers
    // is just a normal "forbidden" (auth, geo-block, etc.) — re-solving won't help.
    let has_cf_markers = (lower.contains("cf-browser-verification")
        || lower.contains("cf_chl_opt")
        || lower.contains("just a moment"))
        && lower.contains("cloudflare");

    if has_cf_markers {
        if lower.contains("cf-browser-verification") {
            return Some("cf-browser-verification");
        }
        if lower.contains("cf_chl_opt") {
            return Some("cf_chl_opt");
        }
        return Some("just a moment");
    }

    // Cloudflare sometimes returns very short 403/503 bodies that lack the usual
    // markers but still contain "cloudflare" in a server header rendered in the
    // page, or a turnstile script. Check for these narrower patterns only on
    // status codes that Cloudflare commonly uses for challenges.
    if (status == 403 || status == 503) && lower.contains("cloudflare") {
        return Some("403/503 status mentioning cloudflare");
    }

    None
}

#[cfg(test)]
fn looks_like_cf_challenge(status: u16, body: &str) -> bool {
    cf_challenge_marker(status, body).is_some()
}

impl FlareClient {
    fn operation(&self) -> Operation {
        Operation::new(self.lock_inner().limiter.clone())
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Re-solve via FlareSolverr and update the internal browser user agent and cookies.
    /// Returns Ok(true) if re-solve succeeded, Ok(false) if no FS configured.
    fn re_solve(&self, attempt: &SolveAttempt, operation: &Operation) -> Result<bool> {
        attempt.run(operation, || self.solve_once(operation))
    }

    // Capture before the direct attempt so even a late challenge response can
    // reuse a solve completed by another request using the same older agent.
    fn pending_solve(&self) -> Arc<SolveAttempt> {
        let mut attempt = self
            .solve_attempt
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if attempt.is_complete() {
            *attempt = Arc::new(SolveAttempt::default());
        }
        attempt.clone()
    }

    fn solve_once(&self, operation: &Operation) -> Result<bool> {
        let (fs_url, origin_url) = {
            let guard = self.lock_inner();
            match &guard.flaresolverr_url {
                Some(url) => (url.clone(), guard.origin_url.clone()),
                None => return Ok(false),
            }
        };
        let session_id = self.session_for_request(operation)?;

        debug!(
            "FlareClient: re-solving challenge via FlareSolverr for {}",
            origin_url
        );

        let solved = match solve_with_flaresolverr(
            &fs_url,
            &origin_url,
            session_id.as_ref().map(|s| s.id.as_str()),
            operation,
        ) {
            Ok(solved) => solved,
            Err(error)
                if session_id.is_some()
                    && self.uses_named_session()
                    && is_missing_flaresolverr_session(&error) =>
            {
                warn!(
                    "FlareClient: session disappeared while re-solving {}, refreshing it: {:#}",
                    origin_url, error
                );
                let failed = session_id
                    .as_ref()
                    .ok_or_else(|| anyhow!("missing managed session lease"))?;
                let refreshed_session = self.refresh_named_session(failed, operation)?;
                solve_with_flaresolverr(
                    &fs_url,
                    &origin_url,
                    Some(&refreshed_session.id),
                    operation,
                )?
            }
            Err(error) => return Err(error),
        };
        let new_agent = build_lenient_ureq_agent(Some(&solved.user_agent));
        insert_flaresolverr_cookies_into_agent(&new_agent, &solved.url, solved.cookies)?;

        {
            let mut guard = self.lock_inner();
            guard.agent = new_agent;
        }

        debug!("FlareClient: re-solve succeeded, agent updated");
        Ok(true)
    }

    /// Construct without network I/O from explicit settings. The environment
    /// adapter belongs to FlareClientConfig; lazy solving stays in the request path.
    pub fn new(config: FlareClientConfig) -> Self {
        init_plugin_logging();
        let mut configuration_error = config.validate_solver_url().err();
        debug!(
            "FlareClient for {}: solver configured={}",
            config.origin_url,
            config.solver_url.is_some()
        );
        let limiter = match config.requests_per_second.map(RateLimiter::new).transpose() {
            Ok(limiter) => limiter.map(Arc::new),
            Err(error) => {
                configuration_error = configuration_error.or(Some(error));
                None
            }
        };
        Self {
            solve_attempt: Arc::default(),
            configuration_error,
            inner: Arc::new(Mutex::new(Inner {
                agent: build_lenient_ureq_agent(Some(DEFAULT_BROWSER_UA)),
                origin_url: config.origin_url,
                flaresolverr_url: config.solver_url,
                session: config.session.into(),
                default_headers: vec![],
                limiter,
                route: RouteState::DirectEligible,
            })),
        }
    }

    fn check_configuration(&self) -> Result<()> {
        if let Some(error) = self.configuration_error {
            log::error!("FlareClient: {error}");
            return Err(anyhow!(error));
        }
        Ok(())
    }

    fn direct_path_state(&self) -> (bool, bool) {
        let mut guard = self.lock_inner();
        let should_try_direct = match guard.route {
            RouteState::DirectEligible => true,
            RouteState::ProxyUntil(until) if Instant::now() >= until => {
                debug!(
                    "FlareClient: direct path cooldown expired for {}, retrying direct",
                    guard.origin_url
                );
                guard.route = RouteState::DirectEligible;
                true
            }
            RouteState::ProxyUntil(until) => {
                debug!(
                    "FlareClient: direct path disabled for {} ({:?} cooldown remaining), going straight to proxy",
                    guard.origin_url,
                    until.saturating_duration_since(Instant::now())
                );
                false
            }
        };

        (should_try_direct, guard.flaresolverr_url.is_some())
    }

    fn disable_direct_path(&self) {
        let mut guard = self.lock_inner();
        guard.route = RouteState::ProxyUntil(Instant::now() + DIRECT_RETRY_COOLDOWN);
    }

    /// Thread-safe: takes &self. Internally locks, mutates as needed.
    ///
    /// Strategy: direct-first, proxy-on-challenge with learning.
    ///   1. If direct requests have worked before (or never been tried), try a
    ///      direct GET using the current agent.
    ///   2. If challenged, lazily re-solve via FlareSolverr and retry direct once.
    ///   3. If direct is still challenged, enter the proxy cooldown so
    ///      future requests skip straight to the proxy, then proxy this request.
    ///   4. After a cooldown, try the direct path again.
    pub fn fetch_text(&self, url: &str) -> Result<String> {
        self.fetch_document(url).map(|document| document.body)
    }

    /// Fetch text through the same recovery ladder, retaining the final URL.
    pub fn fetch_document(&self, url: &str) -> Result<FetchedDocument> {
        self.request_with_ladder(
            "GET",
            url,
            |client, request_url, operation| client.try_direct_get(request_url, operation),
            proxy_fetch_document,
        )
    }

    fn request_with_ladder<D, P>(
        &self,
        method: &str,
        url: &str,
        direct_request: D,
        proxy_request: P,
    ) -> Result<FetchedDocument>
    where
        D: Fn(&Self, &str, &Operation) -> Result<DirectResult>,
        P: Fn(&str, Option<&str>, &str, &Operation) -> Result<FetchedDocument>,
    {
        self.check_configuration()?;
        let budget = self.operation();
        let operation = &budget;
        let (should_try_direct, has_fs) = self.direct_path_state();
        let mut last_error: Option<anyhow::Error> = None;

        if should_try_direct {
            let solve_attempt = self.pending_solve();
            debug!("FlareClient: direct {} {}", method, url);
            match self.direct_with_backoff(url, operation, &direct_request) {
                Ok(DirectResult::Success(text)) => {
                    debug!("FlareClient: direct {} succeeded for {}", method, url);
                    return Ok(text);
                }
                Ok(DirectResult::Challenged(status)) => {
                    debug!(
                        "FlareClient: direct {} got challenged (HTTP {}) for {}",
                        method, status, url
                    );
                    last_error = Some(anyhow!("challenged (HTTP {})", status));
                }
                Ok(DirectResult::HttpError(status, _)) => {
                    // Statuses that WAF/CDN layers use to block clients may
                    // still succeed through re-solve/proxy; anything else
                    // (404, 500, ...) is a real answer from the site.
                    if !(has_fs && should_escalate_status(status)) {
                        return Err(anyhow!(
                            "FlareClient: direct {} returned HTTP {} for {}",
                            method,
                            status,
                            url
                        ));
                    }
                    debug!(
                        "FlareClient: direct {} returned HTTP {} for {}, escalating",
                        method, status, url
                    );
                    last_error = Some(anyhow!("HTTP {}", status));
                }
                Err(e) => {
                    // Transport errors (DNS block, connection reset, ...) can
                    // be direct-path-only; the proxy may still get through.
                    if !has_fs || solve::error_is::<crate::backoff::RateLimited>(&e) {
                        return Err(e);
                    }
                    warn!(
                        "FlareClient: direct {} failed for {}, falling back to proxy: {:#}",
                        method, url, e
                    );
                    last_error = Some(e);
                }
            }

            match self.re_solve(&solve_attempt, operation) {
                Ok(true) => {
                    debug!(
                        "FlareClient: retrying direct {} after re-solve for {}",
                        method, url
                    );
                    match self.direct_with_backoff(url, operation, &direct_request) {
                        Ok(DirectResult::Success(text)) => {
                            debug!(
                                "FlareClient: direct {} succeeded after re-solve for {}",
                                method, url
                            );
                            return Ok(text);
                        }
                        Ok(DirectResult::Challenged(status)) => {
                            warn!(
                                "FlareClient: still challenged (HTTP {}) after re-solve for {} {}",
                                status, method, url
                            );
                            last_error = Some(anyhow!("still challenged (HTTP {})", status));
                        }
                        Ok(DirectResult::HttpError(status, _)) => {
                            if !should_escalate_status(status) {
                                return Err(anyhow!(
                                    "FlareClient: direct {} returned HTTP {} after re-solve for {}",
                                    method,
                                    status,
                                    url
                                ));
                            }
                            debug!(
                                "FlareClient: direct {} returned HTTP {} after re-solve for {}, escalating",
                                method, status, url
                            );
                            last_error = Some(anyhow!("HTTP {} after re-solve", status));
                        }
                        Err(e) => {
                            if solve::error_is::<crate::backoff::RateLimited>(&e) {
                                return Err(e);
                            }
                            warn!(
                                "FlareClient: direct {} failed after re-solve for {}, falling back to proxy: {:#}",
                                method, url, e
                            );
                            last_error = Some(e);
                        }
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        "FlareClient: re-solve failed after direct {} challenge for {}: {:#}",
                        method, url, e
                    );
                    if is_managed_session_error(&e)
                        || solve::error_is::<crate::backoff::RateLimited>(&e)
                    {
                        return Err(e);
                    }
                }
            }

            if has_fs {
                info!(
                    "FlareClient: direct path failed, switching to proxy-only for future requests"
                );
                self.disable_direct_path();
            }
        }

        let fs_url_opt = { self.lock_inner().flaresolverr_url.clone() };
        let session_id_opt = match self.session_for_request(operation) {
            Ok(session_id) => session_id,
            Err(error) => {
                warn!(
                    "FlareClient: could not initialize a persistent session for {} {}: {:#}",
                    method, url, error
                );
                return Err(error);
            }
        };

        if let Some(fs_url) = fs_url_opt {
            debug!("FlareClient: proxying {} {} via FlareSolverr", method, url);
            match self.proxy_with_session_retry(
                &fs_url,
                method,
                url,
                session_id_opt.as_ref(),
                &proxy_request,
                operation,
            ) {
                Ok(text) => return Ok(text),
                Err(e) => {
                    warn!("FlareClient: proxy {} failed for {}: {:#}", method, url, e);
                    last_error = Some(e);
                }
            }
        }

        let base = anyhow!("FlareClient: all {} attempts failed for {}", method, url);
        Err(match last_error {
            Some(e) => e.context(base.to_string()),
            None => base,
        })
    }

    fn direct_with_backoff<D>(
        &self,
        url: &str,
        operation: &Operation,
        request: &D,
    ) -> Result<DirectResult>
    where
        D: Fn(&Self, &str, &Operation) -> Result<DirectResult>,
    {
        let result = request(self, url, operation)?;
        let DirectResult::HttpError(429, delay) = result else {
            return Ok(result);
        };
        operation.backoff(delay)?;
        match request(self, url, operation).context(crate::backoff::RateLimited)? {
            DirectResult::HttpError(status, _) => {
                Err(anyhow::Error::new(crate::backoff::RateLimited)
                    .context(format!("direct retry returned HTTP {status} for {url}")))
            }
            result => Ok(result),
        }
    }

    /// Try a direct GET and classify the result.
    fn try_direct_get(&self, url: &str, operation: &Operation) -> Result<DirectResult> {
        let (default_headers, agent) = {
            let guard = self.lock_inner();
            (guard.default_headers.clone(), guard.agent.clone())
        };

        let req = default_headers
            .iter()
            .fold(agent.get(url), |req, (k, v)| req.header(k, v));
        operation.before_attempt()?;
        classify_direct_response(
            req.config()
                .timeout_global(Some(operation.request_timeout()?))
                .build()
                .call()?,
        )
    }

    pub fn fetch_bytes(&self, url: &str) -> Result<Bytes> {
        self.fetch_bytes_with_wrapper_policy(url, ImageWrapperPolicy::Reject)
    }

    /// Fetch an image with explicit source-specific HTML wrapper handling.
    pub fn fetch_bytes_with_wrapper_policy(
        &self,
        url: &str,
        wrappers: ImageWrapperPolicy,
    ) -> Result<Bytes> {
        self.check_configuration()?;
        let operation = self.operation();
        let bytes = bytes_fetch_impl(
            &mut |url| self.image_response(url, &operation, wrappers),
            url,
        )?;
        operation.remaining()?;
        Ok(bytes)
    }

    fn image_request(&self, url: &str, operation: &Operation) -> Result<HttpResponse> {
        let mut retried = false;
        loop {
            let result = self.image_request_once(url, operation);
            let mut response = if retried {
                result.context(crate::backoff::RateLimited)?
            } else {
                result?
            };
            let status = response.status().as_u16();
            if status == 429 || (retried && status >= 400) {
                let delay = crate::backoff::retry_after(response.headers());
                let is_html = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.to_ascii_lowercase().starts_with("text/html"));
                if is_html
                    && cf_challenge_marker(
                        status,
                        &response
                            .body_mut()
                            .read_to_string()
                            .context(crate::backoff::RateLimited)?,
                    )
                    .is_some()
                {
                    return Err(anyhow!(
                        "Cloudflare challenge (HTTP {status}) while fetching image {url}"
                    ));
                }
                if status == 429 {
                    drop(response);
                    operation.backoff(delay)?;
                    retried = true;
                    continue;
                }
                return Err(anyhow::Error::new(crate::backoff::RateLimited)
                    .context(format!("image retry returned HTTP {status}")));
            }
            return Ok(response);
        }
    }

    /// One throttled image GET with the client's current agent and headers.
    fn image_request_once(&self, url: &str, operation: &Operation) -> Result<HttpResponse> {
        let (default_headers, agent, origin_url) = {
            let guard = self.lock_inner();
            (
                guard.default_headers.clone(),
                guard.agent.clone(),
                guard.origin_url.clone(),
            )
        };
        operation.before_attempt()?;
        let mut req = agent.get(url);
        for (k, v) in default_headers.iter() {
            req = req.header(k, v);
        }
        req = build_image_get_with_referer(url, req, Some(&origin_url));
        Ok(req
            .config()
            .timeout_global(Some(operation.request_timeout()?))
            .build()
            .call()?)
    }

    fn image_response(
        &self,
        url: &str,
        operation: &Operation,
        wrappers: ImageWrapperPolicy,
    ) -> Result<ImageResponse> {
        // A transport error or a block-shaped status (403/503) may just
        // mean expired clearance cookies; re-solve once and retry.
        let solve_attempt = self.pending_solve();
        let (first, blocked) = match self.image_request(url, operation) {
            Err(error) if solve::error_is::<crate::backoff::RateLimited>(&error) => {
                return Err(error);
            }
            Err(error) => (Err(error), true),
            Ok(response) => {
                let blocked_status = should_escalate_status(response.status().as_u16());
                let parsed = parse_image_response(response, url, wrappers);
                let challenged = parsed
                    .as_ref()
                    .err()
                    .is_some_and(|e| e.is::<ImageChallenge>());
                (parsed, blocked_status || challenged)
            }
        };

        let solved = if blocked {
            match self.re_solve(&solve_attempt, operation) {
                Ok(solved) => solved,
                Err(error)
                    if is_managed_session_error(&error)
                        || solve::error_is::<crate::backoff::RateLimited>(&error) =>
                {
                    warn!(
                        "FlareClient: image session recovery failed for {}: {:#}",
                        url, error
                    );
                    return Err(error);
                }
                Err(_) => false,
            }
        } else {
            false
        };
        if solved {
            debug!(
                "FlareClient: retrying image fetch after re-solve for {}",
                url
            );
            parse_image_response(self.image_request(url, operation)?, url, wrappers)
        } else {
            first
        }
    }

    pub fn post_form_text(&self, url: &str, form: &[(&str, &str)]) -> Result<String> {
        self.post_form_document(url, form)
            .map(|document| document.body)
    }

    /// Submit a form, retaining the final response URL and text body.
    pub fn post_form_document(&self, url: &str, form: &[(&str, &str)]) -> Result<FetchedDocument> {
        self.request_with_ladder(
            "POST",
            url,
            |client, request_url, operation| {
                client.try_direct_post_form(request_url, form, operation)
            },
            |fs_url, session_id, request_url, operation| {
                proxy_post_form(fs_url, session_id, request_url, form, operation)
            },
        )
    }

    /// Try a direct POST with form data and classify the result.
    fn try_direct_post_form(
        &self,
        url: &str,
        form: &[(&str, &str)],
        operation: &Operation,
    ) -> Result<DirectResult> {
        let (default_headers, agent) = {
            let guard = self.lock_inner();
            (guard.default_headers.clone(), guard.agent.clone())
        };

        let mut req = agent.post(url);
        for (k, v) in default_headers.iter() {
            req = req.header(k, v);
        }
        operation.before_attempt()?;
        classify_direct_response(
            req.config()
                .timeout_global(Some(operation.request_timeout()?))
                .build()
                .send_form(form.iter().copied())?,
        )
    }

    fn try_direct_post_empty(
        &self,
        url: &str,
        extra_headers: &[(&str, &str)],
        operation: &Operation,
    ) -> Result<DirectResult> {
        let (default_headers, agent) = {
            let guard = self.lock_inner();
            (guard.default_headers.clone(), guard.agent.clone())
        };

        let mut req = agent.post(url);
        for (k, v) in default_headers.iter() {
            req = req.header(k, v);
        }
        for (k, v) in extra_headers.iter() {
            req = req.header(*k, *v);
        }

        operation.before_attempt()?;
        classify_direct_response(
            req.config()
                .timeout_global(Some(operation.request_timeout()?))
                .build()
                .send_empty()?,
        )
    }

    pub fn post_empty_text(&self, url: &str, extra_headers: &[(&str, &str)]) -> Result<String> {
        self.post_empty_document(url, extra_headers)
            .map(|document| document.body)
    }

    /// Submit an empty POST, retaining the final response URL and text body.
    pub fn post_empty_document(
        &self,
        url: &str,
        extra_headers: &[(&str, &str)],
    ) -> Result<FetchedDocument> {
        // FlareSolverr's request.post proxy leg cannot carry these per-request
        // headers, so they apply only to the direct request.
        self.request_with_ladder(
            "POST",
            url,
            |client, request_url, operation| {
                client.try_direct_post_empty(request_url, extra_headers, operation)
            },
            proxy_post_empty,
        )
    }
}

fn classify_direct_response(mut resp: HttpResponse) -> Result<DirectResult> {
    let status = resp.status().as_u16();
    let delay = crate::backoff::retry_after(resp.headers());
    let final_url = resp.get_uri().to_string();
    let body = match resp.body_mut().read_to_string() {
        Ok(body) => body,
        Err(error) if status == 429 => {
            warn!(
                "FlareClient: could not read HTTP 429 response body; preserving rate-limit backoff: {:#}",
                error
            );
            return Ok(DirectResult::HttpError(status, delay));
        }
        Err(error) => return Err(error.into()),
    };

    if let Some(marker) = cf_challenge_marker(status, &body) {
        debug!(
            "FlareClient: HTTP {} ({} bytes) classified as challenge, marker: {}",
            status,
            body.len(),
            marker
        );
        Ok(DirectResult::Challenged(status))
    } else if status >= 400 {
        debug!(
            "FlareClient: HTTP {} ({} bytes) classified as http error",
            status,
            body.len()
        );
        Ok(DirectResult::HttpError(status, delay))
    } else {
        debug!(
            "FlareClient: HTTP {} ({} bytes) classified as success",
            status,
            body.len()
        );
        Ok(DirectResult::Success(FetchedDocument { final_url, body }))
    }
}

/// Statuses that WAF/CDN layers commonly use to block a client rather than
/// answer the request; worth retrying through re-solve/proxy instead of
/// failing immediately.
fn should_escalate_status(status: u16) -> bool {
    matches!(status, 403 | 503)
}

/// Result of a direct HTTP request classified by challenge detection.
enum DirectResult {
    /// Normal response body.
    Success(FetchedDocument),
    /// Cloudflare challenge detected; carries the HTTP status code.
    Challenged(u16),
    /// A non-challenge HTTP error response.
    HttpError(u16, Duration),
}

#[cfg(test)]
mod test;
