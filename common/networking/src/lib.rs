mod ratelimit;

use anyhow::{Context, Result, anyhow};
use bytes::Bytes;
use cookie::time::OffsetDateTime as CookieOffsetDateTime;
use cookie_store as _;
use log::{debug, info, warn};
use ratelimit::RateLimiter;
use scraper::{Html, Selector};
use serde_json::{Value as JsonValue, json};
use std::error::Error;
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};
use ureq::typestate::{WithBody, WithoutBody};
use ureq::{
    Cookie,
    http::{Uri, header::CONTENT_TYPE},
};
use url::Url;

pub type Agent = ureq::Agent;

pub type HttpResponse = ureq::http::Response<ureq::Body>;

const LIMIT_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB
const DIRECT_RETRY_COOLDOWN: Duration = Duration::from_secs(15 * 60);

// Session discovery/creation is rare, but multiple extension instances can
// race to initialize the same named session. Serialize that small critical
// section so we do not issue duplicate sessions.create calls.
static FLARESOLVERR_SESSION_INIT_LOCK: Mutex<()> = Mutex::new(());

/// User agent for FlareClient agents before the first FlareSolverr solve
/// replaces them with the solved browser's UA. Sites that block default
/// library user agents outright would otherwise 403 every first contact.
const DEFAULT_BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:132.0) Gecko/20100101 Firefox/132.0";

/// Install a logger for code running inside an extension, once, the first
/// time a client is built. Extensions are cdylibs: the host application's
/// logger never reaches the plugin's own `log` facade, so without this every
/// log line in extension code is silently dropped. Controlled by RUST_LOG
/// (e.g. RUST_LOG=networking=debug), defaults to info, writes to stderr.
pub fn init_plugin_logging() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .try_init();
    });
}

#[derive(Clone)]
pub struct RateLimitedAgent {
    inner: ureq::Agent,
    limiter: Option<Arc<RateLimiter>>,
}

impl RateLimitedAgent {
    pub fn new(inner: ureq::Agent, requests_per_second: Option<f64>) -> Self {
        init_plugin_logging();
        debug!(
            "Net RateLimitedAgent setup with {:?} RPS",
            requests_per_second
        );
        let limiter = requests_per_second.and_then(RateLimiter::new).map(Arc::new);

        Self { inner, limiter }
    }

    pub fn get(&self, url: &str) -> RateLimitedRequest<WithoutBody> {
        RateLimitedRequest {
            inner: self.inner.get(url),
            limiter: self.limiter.clone(),
            url: url.to_string(),
        }
    }

    pub fn post(&self, url: &str) -> RateLimitedRequest<WithBody> {
        RateLimitedRequest {
            inner: self.inner.post(url),
            limiter: self.limiter.clone(),
            url: url.to_string(),
        }
    }

    /// Perform a throttled GET and read the response body as text.
    pub fn fetch_text(&self, url: &str) -> Result<String> {
        let mut response = self.get(url).call()?;
        Ok(response.body_mut().read_to_string()?)
    }

    pub fn fetch_bytes(&self, url: &str, referer: Option<&str>) -> anyhow::Result<Bytes> {
        let mut getter = |u: &str| -> anyhow::Result<ureq::http::Response<ureq::Body>> {
            Ok(self.get(u).image_defaults_with_referer(u, referer).call()?)
        };

        bytes_fetch_impl(&mut getter, url, 0)
    }
}

pub struct RateLimitedRequest<B> {
    inner: ureq::RequestBuilder<B>,
    limiter: Option<Arc<RateLimiter>>,
    url: String,
}

impl<B> RateLimitedRequest<B> {
    #[inline]
    fn throttle(&self) {
        if let Some(l) = &self.limiter {
            l.acquire();
        }
    }

    pub fn header<K, V>(self, key: K, value: V) -> Self
    where
        ureq::http::header::HeaderName: TryFrom<K>,
        <ureq::http::header::HeaderName as TryFrom<K>>::Error: Into<ureq::http::Error>,
        ureq::http::header::HeaderValue: TryFrom<V>,
        <ureq::http::header::HeaderValue as TryFrom<V>>::Error: Into<ureq::http::Error>,
    {
        Self {
            inner: self.inner.header(key, value),
            limiter: self.limiter,
            url: self.url,
        }
    }

    pub fn query<K, V>(self, key: K, value: V) -> Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        Self {
            inner: self.inner.query(key, value),
            limiter: self.limiter,
            url: self.url,
        }
    }
}

/// Methods only available for RequestBuilder<WithoutBody>
impl RateLimitedRequest<WithoutBody> {
    pub fn image_defaults(self, url: &str) -> Self {
        let inner = build_image_get(url, self.inner);
        Self {
            inner,
            limiter: self.limiter,
            url: self.url,
        }
    }

    fn image_defaults_with_referer(self, url: &str, referer: Option<&str>) -> Self {
        let inner = build_image_get_with_referer(url, self.inner, referer);
        Self {
            inner,
            limiter: self.limiter,
            url: self.url,
        }
    }

    pub fn call(self) -> Result<HttpResponse, ureq::Error> {
        self.throttle();
        debug!("GET {}", self.url);
        self.inner.call()
    }
}

/// Methods only available for RequestBuilder<WithBody>
impl RateLimitedRequest<WithBody> {
    pub fn send_empty(self) -> Result<HttpResponse, ureq::Error> {
        self.throttle();
        debug!("POST (empty) {}", self.url);
        self.inner.send_empty()
    }

    pub fn send_form<I, K, V>(self, form: I) -> Result<HttpResponse, ureq::Error>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        self.throttle();
        debug!("POST (form) {}", self.url);
        self.inner.send_form(form)
    }

    pub fn send_json<T: serde::Serialize>(self, value: &T) -> Result<HttpResponse, ureq::Error> {
        self.throttle();
        debug!("POST (json) {}", self.url);
        self.inner.send_json(value)
    }
}

#[allow(non_snake_case)]
#[derive(Debug, serde::Deserialize, Clone)]
pub struct FlareSolverrResponse {
    pub status: String,
    pub message: String,
    pub solution: FlareSolverrSolution,
    pub startTimestamp: u64,
    pub endTimestamp: u64,
    pub version: String,
}

#[allow(non_snake_case)]
#[derive(Debug, serde::Deserialize, Clone)]
pub struct FlareSolverrSolution {
    pub url: String,
    pub status: u16,
    pub cookies: Vec<FlareSolverrCookie>,
    pub userAgent: String,
    pub headers: JsonValue,
    pub response: String,
}

#[allow(non_snake_case)]
#[derive(Debug, serde::Deserialize, Clone)]
pub struct FlareSolverrCookie {
    pub domain: String,
    pub expiry: Option<u64>,
    pub httpOnly: bool,
    pub name: String,
    pub path: String,
    pub sameSite: String,
    pub secure: bool,
    pub value: String,
}

pub fn build_ureq_agent(user_agent: Option<&str>) -> Agent {
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
fn build_lenient_ureq_agent(user_agent: Option<&str>) -> Agent {
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

pub fn build_rate_limited_flaresolverr_client(
    origin_url: &str,
    requests_per_second: Option<f64>,
) -> FlareClient {
    FlareClient::from_env_with_rps(origin_url, requests_per_second)
        .unwrap_or_else(|_| FlareClient::plain_with_rps(requests_per_second))
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
    FlareClient::from_env_with_rps_and_session(origin_url, requests_per_second, Some(&session_name))
        .unwrap_or_else(|_| FlareClient::plain_with_rps(requests_per_second))
}

fn insert_flaresolverr_cookies_into_agent(agent: &Agent, cookies: Vec<FlareSolverrCookie>) {
    let mut jar = agent.cookie_jar_lock();
    for c in cookies {
        let mut parts = vec![format!("{}={}", c.name, c.value)];

        // Path
        if !c.path.is_empty() {
            parts.push(format!("Path={}", c.path));
        }
        // Domain
        if !c.domain.is_empty() {
            parts.push(format!("Domain={}", c.domain));
        }
        // Secure / HttpOnly
        if c.secure {
            parts.push("Secure".to_string());
        }
        if c.httpOnly {
            parts.push("HttpOnly".to_string());
        }
        // SameSite
        match c.sameSite.as_str() {
            "Strict" | "Lax" | "None" => parts.push(format!("SameSite={}", c.sameSite)),
            _ => {}
        }
        // Expires
        if let Some(expiry) = c.expiry
            && let Ok(ts) = CookieOffsetDateTime::from_unix_timestamp(expiry as i64)
        {
            let max_age = ts.unix_timestamp() - CookieOffsetDateTime::now_utc().unix_timestamp();
            if max_age > 0 {
                parts.push(format!("Max-Age={}", max_age));
            }
        }

        let set_cookie = parts.join("; ");

        // Bind parse to a relevant URI (scheme/host are used for defaults)
        let uri_str = if c.domain.starts_with("http://") || c.domain.starts_with("https://") {
            c.domain.clone()
        } else {
            format!("https://{}", c.domain)
        };
        // Fallback: if domain is empty, use a dummy host.
        let uri = Uri::try_from(uri_str.as_str())
            .unwrap_or_else(|_| Uri::from_static("https://example.com"));

        if let Ok(cookie) = Cookie::parse(set_cookie, &uri) {
            let _ = jar.insert(cookie, &uri);
        }
    }
    jar.release();
}

pub fn build_flaresolverr_client(
    url: &str,
    flaresolverr_url: &str,
) -> Result<Agent, Box<dyn Error>> {
    let payload = json!({
        "cmd": "request.get",
        "url": url,
        "maxTimeout": 60000,
    });

    let solution = request_flaresolverr(flaresolverr_url, &payload)?;
    let agent = build_ureq_agent(Some(&solution.userAgent));
    insert_flaresolverr_cookies_into_agent(&agent, solution.cookies);

    Ok(agent)
}

#[derive(Debug, serde::Deserialize)]
struct FlareSolverrSessionListResponse {
    #[serde(default)]
    sessions: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
struct FlareSolverrSessionCreateResponse {
    session: Option<String>,
}

fn list_flaresolverr_sessions(flaresolverr_url: &str) -> Result<Vec<String>> {
    let payload = json!({"cmd": "sessions.list"});
    let body: FlareSolverrSessionListResponse =
        serde_json::from_value(flaresolverr_rpc(flaresolverr_url, &payload)?)?;
    Ok(body.sessions)
}

fn create_flaresolverr_session(flaresolverr_url: &str, session_name: &str) -> Result<String> {
    let payload = json!({
        "cmd": "sessions.create",
        "session": session_name,
    });
    let body: FlareSolverrSessionCreateResponse =
        serde_json::from_value(flaresolverr_rpc(flaresolverr_url, &payload)?)?;
    body.session.ok_or_else(|| {
        anyhow!("FlareSolverr sessions.create succeeded without returning a session ID")
    })
}

#[derive(Debug)]
struct MissingFlareSolverrSession(String);

impl std::fmt::Display for MissingFlareSolverrSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FlareSolverr error: {}", self.0)
    }
}

impl Error for MissingFlareSolverrSession {}

fn is_missing_flaresolverr_session(error: &anyhow::Error) -> bool {
    error.downcast_ref::<MissingFlareSolverrSession>().is_some()
}

/// Read the RPC envelope even on HTTP errors: FlareSolverr sends useful error
/// messages with HTTP 500 and without a solution. Interpret messages only here.
fn flaresolverr_rpc(fs_url: &str, payload: &JsonValue) -> Result<JsonValue> {
    let command = payload["cmd"].as_str().unwrap_or("unknown command");
    let mut response = ureq::post(fs_url)
        .config()
        .http_status_as_error(false)
        .build()
        .header("Content-Type", "application/json")
        .send_json(payload)
        .with_context(|| format!("FlareSolverr {command} RPC failed"))?;
    let status = response.status();
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
fn request_flaresolverr(fs_url: &str, payload: &JsonValue) -> Result<FlareSolverrSolution> {
    let mut envelope = flaresolverr_rpc(fs_url, payload)?;
    let solution: FlareSolverrSolution = serde_json::from_value(envelope["solution"].take())
        .context("FlareSolverr returned an invalid or missing solution")?;
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

/// Internal, mutable state wrapped by a Mutex.
#[derive(Clone)]
struct Inner {
    agent: Agent,
    origin_url: String,
    flaresolverr_url: Option<String>,
    session_id: Option<String>,
    /// Named session to create/reuse when no explicit FLARESOLVERR_SESSION
    /// override was provided.
    session_name: Option<String>,
    default_headers: Vec<(String, String)>,
    limiter: Option<Arc<RateLimiter>>,
    /// Tracks whether direct requests work for this site.
    /// Starts `true` (optimistic). Flips to `false` after a challenged
    /// direct+re-solve cycle fails, so subsequent requests skip straight
    /// to the FlareSolverr proxy without wasting round-trips.
    direct_works: bool,
    /// When direct requests may be tried again after a failed challenge cycle.
    direct_disabled_until: Option<Instant>,
}

/// Public handle that is Send + Sync.
#[derive(Clone)]
pub struct FlareClient {
    inner: Arc<Mutex<Inner>>,
}

/// Heuristic: does this response body look like a Cloudflare challenge page?
/// Returns the marker that matched so routing decisions can be traced in logs.
fn cf_challenge_marker(status: u16, body: &str) -> Option<&'static str> {
    let lower = body.to_ascii_lowercase();

    // Cloudflare challenge pages contain characteristic markers.
    // We require at least one challenge-specific marker AND the word "cloudflare"
    // in the body, even for 403/503 status codes. A bare 403 without CF markers
    // is just a normal "forbidden" (auth, geo-block, etc.) — re-solving won't help.
    let has_cf_markers = (lower.contains("cf-browser-verification")
        || lower.contains("cf_chl_opt")
        || lower.contains("challenge-platform")
        || lower.contains("just a moment"))
        && lower.contains("cloudflare");

    if has_cf_markers {
        if lower.contains("cf-browser-verification") {
            return Some("cf-browser-verification");
        }
        if lower.contains("cf_chl_opt") {
            return Some("cf_chl_opt");
        }
        if lower.contains("challenge-platform") {
            return Some("challenge-platform");
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
    #[inline]
    fn throttle(&self) {
        let limiter = {
            let guard = self.lock_inner();
            guard.limiter.clone()
        };
        if let Some(l) = limiter {
            l.acquire();
        }
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn session_for_request(&self) -> Result<Option<String>> {
        let existing = { self.lock_inner().session_id.clone() };
        if existing.is_some() {
            return Ok(existing);
        }
        self.ensure_named_session()
    }

    /// Resolve the configured named session exactly once per FlareClient.
    /// Multiple clients use a process-wide lock so they can safely converge on
    /// the same extension-scoped session without racing sessions.create.
    fn ensure_named_session(&self) -> Result<Option<String>> {
        let (flaresolverr_url, session_name, session_id) = {
            let guard = self.lock_inner();
            (
                guard.flaresolverr_url.clone(),
                guard.session_name.clone(),
                guard.session_id.clone(),
            )
        };

        if session_id.is_some() {
            return Ok(session_id);
        }
        let (Some(flaresolverr_url), Some(session_name)) = (flaresolverr_url, session_name) else {
            return Ok(None);
        };

        let _init_guard = FLARESOLVERR_SESSION_INIT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Another FlareClient may have initialized this shared session while
        // we waited for the process-wide lock.
        {
            let guard = self.lock_inner();
            if guard.session_id.is_some() {
                return Ok(guard.session_id.clone());
            }
        }

        let session_id = if list_flaresolverr_sessions(&flaresolverr_url)?
            .iter()
            .any(|session| session == &session_name)
        {
            debug!(
                "FlareClient: reusing existing FlareSolverr session {}",
                session_name
            );
            session_name
        } else {
            info!(
                "FlareClient: creating FlareSolverr session {}",
                session_name
            );
            create_flaresolverr_session(&flaresolverr_url, &session_name)?
        };

        let mut guard = self.lock_inner();
        if guard.session_id.is_none() {
            guard.session_id = Some(session_id.clone());
        }
        Ok(guard.session_id.clone())
    }

    fn uses_named_session(&self) -> bool {
        self.lock_inner().session_name.is_some()
    }

    fn clear_named_session(&self) {
        let mut guard = self.lock_inner();
        if guard.session_name.is_some() {
            guard.session_id = None;
        }
    }

    fn proxy_with_session_retry<P>(
        &self,
        flaresolverr_url: &str,
        method: &str,
        url: &str,
        session_id: Option<&str>,
        proxy_request: &P,
    ) -> Result<String>
    where
        P: Fn(&str, Option<&str>, &str) -> Result<String>,
    {
        match proxy_request(flaresolverr_url, session_id, url) {
            Ok(text) => Ok(text),
            Err(error)
                if session_id.is_some()
                    && self.uses_named_session()
                    && is_missing_flaresolverr_session(&error) =>
            {
                warn!(
                    "FlareClient: session disappeared while proxying {} {}, refreshing it: {:#}",
                    method, url, error
                );
                self.clear_named_session();
                let refreshed_session = self.ensure_named_session()?.ok_or_else(|| {
                    anyhow!(
                        "FlareClient: named FlareSolverr session unavailable for {} {}",
                        method,
                        url
                    )
                })?;
                proxy_request(flaresolverr_url, Some(&refreshed_session), url)
            }
            Err(error) => Err(error),
        }
    }

    /// Re-solve via FlareSolverr and update the internal agent + headers.
    /// Returns Ok(true) if re-solve succeeded, Ok(false) if no FS configured.
    fn re_solve(&self) -> Result<bool> {
        let (fs_url, origin_url) = {
            let guard = self.lock_inner();
            match &guard.flaresolverr_url {
                Some(url) => (url.clone(), guard.origin_url.clone()),
                None => return Ok(false),
            }
        };
        let session_id = self.session_for_request()?;

        debug!(
            "FlareClient: re-solving challenge via FlareSolverr for {}",
            origin_url
        );

        let solved = match solve_with_flaresolverr(&fs_url, &origin_url, session_id.as_deref()) {
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
                self.clear_named_session();
                let refreshed_session = self.ensure_named_session()?.ok_or_else(|| {
                    anyhow!(
                        "FlareClient: named FlareSolverr session unavailable for {}",
                        origin_url
                    )
                })?;
                solve_with_flaresolverr(&fs_url, &origin_url, Some(&refreshed_session))?
            }
            Err(error) => return Err(error),
        };
        let new_agent = build_lenient_ureq_agent(Some(&solved.user_agent));
        insert_flaresolverr_cookies_into_agent(&new_agent, solved.cookies);

        {
            let mut guard = self.lock_inner();
            guard.agent = new_agent;
            guard.default_headers = solved.headers;
        }

        debug!("FlareClient: re-solve succeeded, agent updated");
        Ok(true)
    }

    pub fn plain_with_rps(requests_per_second: Option<f64>) -> Self {
        init_plugin_logging();
        let limiter = requests_per_second.and_then(RateLimiter::new).map(Arc::new);

        FlareClient {
            inner: Arc::new(Mutex::new(Inner {
                agent: build_lenient_ureq_agent(Some(DEFAULT_BROWSER_UA)),
                origin_url: String::new(),
                flaresolverr_url: None,
                session_id: None,
                session_name: None,
                default_headers: vec![],
                limiter,
                direct_works: true,
                direct_disabled_until: None,
            })),
        }
    }

    pub fn from_env_with_rps(origin_url: &str, requests_per_second: Option<f64>) -> Result<Self> {
        Self::from_env_with_rps_and_session(origin_url, requests_per_second, None)
    }

    /// Construct a client with an optional named FlareSolverr session.
    ///
    /// The session is initialized lazily, after a request actually needs
    /// FlareSolverr. An explicit FLARESOLVERR_SESSION always takes precedence
    /// and is never created, refreshed, or destroyed by this client.
    pub fn from_env_with_rps_and_session(
        origin_url: &str,
        requests_per_second: Option<f64>,
        session_name: Option<&str>,
    ) -> Result<Self> {
        init_plugin_logging();
        let limiter = requests_per_second.and_then(RateLimiter::new).map(Arc::new);

        let flaresolverr_url = std::env::var("FLARESOLVERR_URL").ok();
        debug!(
            "FlareClient for {}: FLARESOLVERR_URL={:?}",
            origin_url, flaresolverr_url
        );

        if flaresolverr_url.is_none() {
            return Ok(Self {
                inner: Arc::new(Mutex::new(Inner {
                    agent: build_lenient_ureq_agent(Some(DEFAULT_BROWSER_UA)),
                    origin_url: origin_url.to_string(),
                    flaresolverr_url: None,
                    session_id: None,
                    session_name: session_name
                        .filter(|name| !name.trim().is_empty())
                        .map(str::to_string),
                    default_headers: vec![],
                    limiter,
                    direct_works: true,
                    direct_disabled_until: None,
                })),
            });
        }

        let flaresolverr_url = flaresolverr_url.unwrap();
        let session_id = std::env::var("FLARESOLVERR_SESSION")
            .ok()
            .filter(|session| !session.trim().is_empty());
        let session_name = if session_id.is_none() {
            session_name
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string)
        } else {
            None
        };

        // Solve lazily on the first challenge instead of blocking construction.
        let agent = build_lenient_ureq_agent(Some(DEFAULT_BROWSER_UA));
        let default_headers = vec![];

        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                agent,
                origin_url: origin_url.to_string(),
                flaresolverr_url: Some(flaresolverr_url),
                session_id,
                session_name,
                default_headers,
                limiter,
                direct_works: true,
                direct_disabled_until: None,
            })),
        })
    }

    /// Plain client (no FlareSolverr), safe default.
    pub fn plain() -> Self {
        Self::plain_with_rps(None)
    }

    /// Make this never error: on any failure, return a plain client.
    pub fn from_env_or_plain(origin_url: &str) -> Self {
        Self::from_env_with_rps(origin_url, None).unwrap_or_else(|_| Self::plain_with_rps(None))
    }

    pub fn from_env(origin_url: &str) -> Result<Self> {
        Self::from_env_with_rps(origin_url, None)
    }

    fn direct_path_state(&self) -> (bool, bool) {
        let mut guard = self.lock_inner();
        let should_try_direct = if guard.direct_works {
            true
        } else if guard
            .direct_disabled_until
            .is_some_and(|until| Instant::now() >= until)
        {
            debug!(
                "FlareClient: direct path cooldown expired for {}, retrying direct",
                guard.origin_url
            );
            guard.direct_works = true;
            guard.direct_disabled_until = None;
            true
        } else {
            let remaining = guard
                .direct_disabled_until
                .map(|until| until.saturating_duration_since(Instant::now()));
            debug!(
                "FlareClient: direct path disabled for {} ({:?} cooldown remaining), going straight to proxy",
                guard.origin_url, remaining
            );
            false
        };

        (should_try_direct, guard.flaresolverr_url.is_some())
    }

    fn disable_direct_path(&self) {
        let mut guard = self.lock_inner();
        guard.direct_works = false;
        guard.direct_disabled_until = Some(Instant::now() + DIRECT_RETRY_COOLDOWN);
    }

    /// Thread-safe: takes &self. Internally locks, mutates as needed.
    ///
    /// Strategy: direct-first, proxy-on-challenge with learning.
    ///   1. If direct requests have worked before (or never been tried), try a
    ///      direct GET using the current agent.
    ///   2. If challenged, lazily re-solve via FlareSolverr and retry direct once.
    ///   3. If direct is still challenged, mark `direct_works = false` so
    ///      future requests skip straight to the proxy, then proxy this request.
    ///   4. After a cooldown, try the direct path again.
    pub fn fetch_text(&self, url: &str) -> Result<String> {
        self.request_with_ladder(
            "GET",
            url,
            |client, request_url| client.try_direct_get(request_url),
            proxy_fetch_text,
        )
    }

    fn request_with_ladder<D, P>(
        &self,
        method: &str,
        url: &str,
        direct_request: D,
        proxy_request: P,
    ) -> Result<String>
    where
        D: Fn(&Self, &str) -> Result<DirectResult>,
        P: Fn(&str, Option<&str>, &str) -> Result<String>,
    {
        self.throttle();
        let (should_try_direct, has_fs) = self.direct_path_state();
        let mut last_error: Option<anyhow::Error> = None;

        if should_try_direct {
            debug!("FlareClient: direct {} {}", method, url);
            match direct_request(self, url) {
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
                Ok(DirectResult::HttpError(status)) => {
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
                    if !has_fs {
                        return Err(e);
                    }
                    warn!(
                        "FlareClient: direct {} failed for {}, falling back to proxy: {:#}",
                        method, url, e
                    );
                    last_error = Some(e);
                }
            }

            match self.re_solve() {
                Ok(true) => {
                    debug!(
                        "FlareClient: retrying direct {} after re-solve for {}",
                        method, url
                    );
                    match direct_request(self, url) {
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
                        Ok(DirectResult::HttpError(status)) => {
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
        let session_id_opt = match self.session_for_request() {
            Ok(session_id) => session_id,
            Err(error) => {
                warn!(
                    "FlareClient: could not initialize a persistent session for {} {}, using a temporary FlareSolverr request: {:#}",
                    method, url, error
                );
                None
            }
        };

        if let Some(fs_url) = fs_url_opt {
            debug!("FlareClient: proxying {} {} via FlareSolverr", method, url);
            match self.proxy_with_session_retry(
                &fs_url,
                method,
                url,
                session_id_opt.as_deref(),
                &proxy_request,
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

    /// Try a direct GET and classify the result.
    fn try_direct_get(&self, url: &str) -> Result<DirectResult> {
        let (default_headers, agent) = {
            let guard = self.lock_inner();
            (guard.default_headers.clone(), guard.agent.clone())
        };

        let req = default_headers
            .iter()
            .fold(agent.get(url), |req, (k, v)| req.header(k, v));
        classify_direct_response(req.call()?)
    }

    pub fn get_text(&self, url: &str) -> Result<String> {
        self.fetch_text(url)
    }

    pub fn fetch_bytes(&self, url: &str) -> Result<Bytes> {
        self.fetch_bytes_inner(url, 0)
    }

    /// One throttled image GET with the client's current agent and headers.
    fn image_request(&self, url: &str) -> Result<HttpResponse, ureq::Error> {
        let (default_headers, agent, origin_url) = {
            let guard = self.lock_inner();
            (
                guard.default_headers.clone(),
                guard.agent.clone(),
                guard.origin_url.clone(),
            )
        };

        self.throttle();
        let mut req = agent.get(url);
        for (k, v) in default_headers.iter() {
            req = req.header(k, v);
        }
        req = build_image_get_with_referer(url, req, Some(&origin_url));
        req.call()
    }

    fn fetch_bytes_inner(&self, url: &str, depth: u8) -> Result<Bytes> {
        if depth > 2 {
            return Err(anyhow!(
                "Too many wrapper hops while fetching image: {}",
                url
            ));
        }

        // A transport error or a block-shaped status (403/429/503) may just
        // mean expired clearance cookies; re-solve once and retry.
        let first = self.image_request(url);
        let blocked = match &first {
            Ok(resp) => should_escalate_status(resp.status().as_u16()),
            Err(_) => true,
        };

        let resp = if blocked && self.re_solve().unwrap_or(false) {
            debug!(
                "FlareClient: retrying image fetch after re-solve for {}",
                url
            );
            self.image_request(url)?
        } else {
            first?
        };

        match parse_image_response(resp, url)? {
            ImageResponse::Bytes(bytes) => Ok(bytes),
            ImageResponse::Redirect(next_url) => self.fetch_bytes_inner(&next_url, depth + 1),
        }
    }

    pub fn post_form_text(&self, url: &str, form: &[(&str, &str)]) -> Result<String> {
        self.request_with_ladder(
            "POST",
            url,
            |client, request_url| client.try_direct_post_form(request_url, form),
            |fs_url, session_id, request_url| {
                proxy_post_form(fs_url, session_id, request_url, form)
            },
        )
    }

    /// Try a direct POST with form data and classify the result.
    fn try_direct_post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<DirectResult> {
        let (default_headers, agent) = {
            let guard = self.lock_inner();
            (guard.default_headers.clone(), guard.agent.clone())
        };

        let mut req = agent.post(url);
        for (k, v) in default_headers.iter() {
            req = req.header(k, v);
        }
        classify_direct_response(req.send_form(form.iter().copied())?)
    }

    fn try_direct_post_empty(
        &self,
        url: &str,
        extra_headers: &[(&str, &str)],
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

        classify_direct_response(req.send_empty()?)
    }

    pub fn post_empty_text(&self, url: &str, extra_headers: &[(&str, &str)]) -> Result<String> {
        // FlareSolverr's request.post proxy leg cannot carry these per-request
        // headers, so they apply only to the direct request.
        self.request_with_ladder(
            "POST",
            url,
            |client, request_url| client.try_direct_post_empty(request_url, extra_headers),
            proxy_post_empty,
        )
    }
}

fn classify_direct_response(mut resp: HttpResponse) -> Result<DirectResult> {
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string()?;

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
        Ok(DirectResult::HttpError(status))
    } else {
        debug!(
            "FlareClient: HTTP {} ({} bytes) classified as success",
            status,
            body.len()
        );
        Ok(DirectResult::Success(body))
    }
}

/// Statuses that WAF/CDN layers commonly use to block a client rather than
/// answer the request; worth retrying through re-solve/proxy instead of
/// failing immediately.
fn should_escalate_status(status: u16) -> bool {
    matches!(status, 403 | 429 | 503)
}

/// Result of a direct HTTP request classified by challenge detection.
enum DirectResult {
    /// Normal response body.
    Success(String),
    /// Cloudflare challenge detected; carries the HTTP status code.
    Challenged(u16),
    /// A non-challenge HTTP error response.
    HttpError(u16),
}

fn proxy_fetch_text(fs_url: &str, session_id: Option<&str>, url: &str) -> Result<String> {
    let payload = match session_id {
        Some(sid) => json!({"cmd":"request.get","url":url,"maxTimeout":60000,"session":sid}),
        None => json!({"cmd":"request.get","url":url,"maxTimeout":60000}),
    };

    Ok(request_flaresolverr(fs_url, &payload)?.response)
}

fn proxy_post_form(
    fs_url: &str,
    session_id: Option<&str>,
    url: &str,
    form: &[(&str, &str)],
) -> Result<String> {
    let body = form
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let payload = match session_id {
        Some(sid) => json!({
            "cmd": "request.post",
            "url": url,
            "maxTimeout": 60000,
            "session": sid,
            "postData": body,
        }),
        None => json!({
            "cmd": "request.post",
            "url": url,
            "maxTimeout": 60000,
            "postData": body,
        }),
    };

    Ok(request_flaresolverr(fs_url, &payload)?.response)
}

fn proxy_post_empty(fs_url: &str, session_id: Option<&str>, url: &str) -> Result<String> {
    let payload = match session_id {
        Some(sid) => json!({
            "cmd": "request.post",
            "url": url,
            "maxTimeout": 60000,
            "session": sid,
            "postData": "",
        }),
        None => json!({
            "cmd": "request.post",
            "url": url,
            "maxTimeout": 60000,
            "postData": "",
        }),
    };

    Ok(request_flaresolverr(fs_url, &payload)?.response)
}

struct Solved {
    user_agent: String,
    cookies: Vec<FlareSolverrCookie>,
    headers: Vec<(String, String)>,
}

fn solve_with_flaresolverr(
    flaresolverr_url: &str,
    url: &str,
    session: Option<&str>,
) -> Result<Solved> {
    let payload = match session {
        Some(sid) => json!({"cmd":"request.get","url":url,"maxTimeout":60000,"session":sid}),
        None => json!({"cmd":"request.get","url":url,"maxTimeout":60000}),
    };

    let solution = request_flaresolverr(flaresolverr_url, &payload)?;

    let mut hdrs: Vec<(String, String)> = vec![];
    if let Some(obj) = solution.headers.as_object() {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                if !k.eq_ignore_ascii_case("set-cookie") {
                    hdrs.push((k.to_string(), s.to_string()));
                }
            } else {
                hdrs.push((k.to_string(), v.to_string()));
            }
        }
    }

    Ok(Solved {
        user_agent: solution.userAgent.clone(),
        cookies: solution.cookies,
        headers: hdrs,
    })
}

const IMAGE_ACCEPT: &str = "image/avif,image/webp,image/apng,image/*,*/*;q=0.8";

fn build_image_get(
    url: &str,
    req: ureq::RequestBuilder<WithoutBody>,
) -> ureq::RequestBuilder<WithoutBody> {
    build_image_get_with_referer(url, req, None)
}

fn build_image_get_with_referer(
    url: &str,
    mut req: ureq::RequestBuilder<WithoutBody>,
    referer: Option<&str>,
) -> ureq::RequestBuilder<WithoutBody> {
    let referer = referer
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(|h| format!("{}://{}/", u.scheme(), h)))
        });

    req = req.header("Accept", IMAGE_ACCEPT);

    if let Some(r) = referer {
        req = req.header("Referer", r);
    }

    req
}

/// A parsed image response: either the image bytes, or the URL of the real
/// image when the server answered with an HTML wrapper page.
enum ImageResponse {
    Bytes(Bytes),
    Redirect(String),
}

/// Validate status and content type, then read the body: image bytes pass
/// through, HTML wrapper pages resolve to the first `<img src>` URL. Depth
/// checks, recursion, and retry logic stay with the callers.
fn parse_image_response(mut resp: HttpResponse, url: &str) -> Result<ImageResponse> {
    let status = resp.status();
    if status.as_u16() >= 400 {
        return Err(anyhow!(
            "Image fetch failed: HTTP {} for {}",
            status.as_u16(),
            url
        ));
    }

    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if content_type.starts_with("text/html") {
        let html = resp.body_mut().read_to_string()?;
        if let Some(next) = extract_first_img_src(&html) {
            let next_url = match Url::parse(url).ok().and_then(|base| base.join(&next).ok()) {
                Some(u) => u.to_string(),
                None => next,
            };
            return Ok(ImageResponse::Redirect(next_url));
        }

        return Err(anyhow!(
            "Expected image bytes but got HTML and no <img src=...> found for {}",
            url
        ));
    }

    let data: Vec<u8> = resp
        .body_mut()
        .with_config()
        .limit(LIMIT_BYTES)
        .read_to_vec()?;
    Ok(ImageResponse::Bytes(Bytes::from(data)))
}

fn bytes_fetch_impl<F>(do_get: &mut F, url: &str, depth: u8) -> anyhow::Result<Bytes>
where
    F: FnMut(&str) -> anyhow::Result<ureq::http::Response<ureq::Body>>,
{
    if depth > 2 {
        return Err(anyhow!(
            "Too many wrapper hops while fetching image: {}",
            url
        ));
    }

    let resp = do_get(url)?;
    match parse_image_response(resp, url)? {
        ImageResponse::Bytes(bytes) => Ok(bytes),
        ImageResponse::Redirect(next_url) => bytes_fetch_impl(do_get, &next_url, depth + 1),
    }
}

// Tiny helper: pull the first <img ... src="..."> out of wrapper HTML.
fn extract_first_img_src(html: &str) -> Option<String> {
    let selector = Selector::parse("img[src]").ok()?;
    Html::parse_document(html)
        .select(&selector)
        .find_map(|image| image.value().attr("src").map(str::to_string))
}

#[cfg(test)]
mod test {
    use super::*;
    use serde_json::json;
    use std::env;
    use std::sync::{Mutex as StdMutex, OnceLock};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn flaresolverr_url() -> String {
        env::var("FLARESOLVERR_URL").unwrap_or_else(|_| "http://localhost:8191/v1".to_string())
    }

    fn env_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static ENV_TEST_GUARD: OnceLock<StdMutex<()>> = OnceLock::new();
        ENV_TEST_GUARD
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn get_flaresolverr_response(url: &str, flaresolverr_url: &str) -> FlareSolverrResponse {
        let payload = json!({
            "cmd": "request.get",
            "url": url,
            "maxTimeout": 60000,
        });

        let flare_response = ureq::post(flaresolverr_url)
            .header("Content-Type", "application/json")
            .send_json(&payload);

        assert!(flare_response.is_ok());
        let mut resp = flare_response.unwrap();
        let text = resp.body_mut().read_to_string().unwrap();
        serde_json::from_str::<FlareSolverrResponse>(&text).unwrap()
    }

    fn get_ureq_response(url: &str, flaresolverr_url: &str) -> String {
        let client = build_flaresolverr_client(url, flaresolverr_url).unwrap();
        let resp = client.get(url).call();

        if let Err(e) = &resp {
            eprintln!("Error making request: {}", e);
        }

        assert!(resp.is_ok());
        let mut r = resp.unwrap();
        r.body_mut().read_to_string().unwrap()
    }

    /// Build a mock FlareSolverrCookie for testing.
    fn mock_cookie(name: &str, value: &str, domain: &str) -> FlareSolverrCookie {
        FlareSolverrCookie {
            domain: domain.to_string(),
            expiry: Some((CookieOffsetDateTime::now_utc().unix_timestamp() + 3600) as u64),
            httpOnly: true,
            name: name.to_string(),
            path: "/".to_string(),
            sameSite: "Lax".to_string(),
            secure: true,
            value: value.to_string(),
        }
    }

    // =======================================================================
    // Unit tests — no network / no FlareSolverr needed
    // =======================================================================

    // --- looks_like_cf_challenge -------------------------------------------

    #[test]
    fn test_cf_challenge_detection_403_without_cf_markers() {
        // A bare 403 without any Cloudflare markers is NOT a challenge —
        // it's a normal forbidden response (auth, geo-block, etc.).
        assert!(!looks_like_cf_challenge(403, ""));
        assert!(!looks_like_cf_challenge(403, "some random body"));
        assert!(!looks_like_cf_challenge(403, "<html>Forbidden</html>"));
    }

    #[test]
    fn test_cf_challenge_detection_403_with_cloudflare() {
        // A 403 that mentions "cloudflare" IS treated as a challenge.
        assert!(looks_like_cf_challenge(
            403,
            "<html>Cloudflare: Access denied</html>"
        ));
    }

    #[test]
    fn test_cf_challenge_detection_503_without_cf_markers() {
        // Same for 503 — no Cloudflare markers means not a CF challenge.
        assert!(!looks_like_cf_challenge(503, ""));
        assert!(!looks_like_cf_challenge(503, "<html>maintenance</html>"));
    }

    #[test]
    fn test_cf_challenge_detection_503_with_cloudflare() {
        assert!(looks_like_cf_challenge(
            503,
            "<html>Service Temporarily Unavailable - Cloudflare</html>"
        ));
    }

    #[test]
    fn test_cf_challenge_detection_200_with_challenge_markers() {
        let body = r#"<html><head><title>Just a moment...</title></head>
            <body><div id="cf-browser-verification">Please wait...</div>
            Powered by Cloudflare</body></html>"#;
        assert!(looks_like_cf_challenge(200, body));
    }

    #[test]
    fn test_cf_challenge_detection_200_cf_chl_opt() {
        let body = r#"<html><script>window._cf_chl_opt={/* ... */};</script>
            <noscript>Cloudflare</noscript></html>"#;
        assert!(looks_like_cf_challenge(200, body));
    }

    #[test]
    fn test_cf_challenge_detection_200_challenge_platform() {
        let body = r#"<html><script src="/cdn-cgi/challenge-platform/scripts/jsd/main.js"></script>
            cloudflare</html>"#;
        assert!(looks_like_cf_challenge(200, body));
    }

    #[test]
    fn test_cf_challenge_detection_normal_page_not_flagged() {
        let body = "<html><body><h1>Hello World</h1></body></html>";
        assert!(!looks_like_cf_challenge(200, body));
    }

    #[test]
    fn test_cf_challenge_detection_page_mentioning_cloudflare_without_markers() {
        // Mentions "cloudflare" but none of the challenge-specific markers,
        // so it should NOT be treated as a challenge.
        let body = "<html><body>We use Cloudflare for CDN.</body></html>";
        assert!(!looks_like_cf_challenge(200, body));
    }

    #[test]
    fn test_cf_challenge_detection_case_insensitive() {
        let body = r#"<html><body>JUST A MOMENT... CLOUDFLARE</body></html>"#;
        assert!(looks_like_cf_challenge(200, body));
    }

    // --- extract_first_img_src ---------------------------------------------

    #[test]
    fn test_extract_img_src_basic() {
        let html = r#"<html><body><img src="https://example.com/image.png"></body></html>"#;
        assert_eq!(
            extract_first_img_src(html),
            Some("https://example.com/image.png".to_string())
        );
    }

    #[test]
    fn test_extract_img_src_relative() {
        let html = r#"<img src="/images/foo.jpg" alt="foo">"#;
        assert_eq!(
            extract_first_img_src(html),
            Some("/images/foo.jpg".to_string())
        );
    }

    #[test]
    fn test_extract_img_src_no_img() {
        let html = "<html><body>No images here</body></html>";
        assert_eq!(extract_first_img_src(html), None);
    }

    #[test]
    fn test_extract_img_src_picks_first() {
        let html = r#"
            <script src="not-an-image.js"></script>
            <iframe src="not-an-image.html"></iframe>
            <img src="first.png"><img src="second.png">
        "#;
        assert_eq!(extract_first_img_src(html), Some("first.png".to_string()));
    }

    // --- FlareSolverrResponse deserialization --------------------------------

    #[test]
    fn test_flaresolverr_response_deserialization() {
        let json_str = r#"{
            "status": "ok",
            "message": "Challenge solved!",
            "startTimestamp": 1700000000000,
            "endTimestamp": 1700000005000,
            "version": "3.3.21",
            "solution": {
                "url": "https://example.com",
                "status": 200,
                "cookies": [
                    {
                        "domain": ".example.com",
                        "expiry": 1700003600,
                        "httpOnly": true,
                        "name": "cf_clearance",
                        "path": "/",
                        "sameSite": "None",
                        "secure": true,
                        "value": "abc123"
                    }
                ],
                "userAgent": "Mozilla/5.0 Test Agent",
                "headers": {
                    "Content-Type": "text/html",
                    "X-Custom": "value"
                },
                "response": "<html>solved page</html>"
            }
        }"#;

        let parsed: FlareSolverrResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed.status, "ok");
        assert_eq!(parsed.message, "Challenge solved!");
        assert_eq!(parsed.version, "3.3.21");
        assert_eq!(parsed.solution.url, "https://example.com");
        assert_eq!(parsed.solution.status, 200);
        assert_eq!(parsed.solution.userAgent, "Mozilla/5.0 Test Agent");
        assert_eq!(parsed.solution.cookies.len(), 1);
        assert_eq!(parsed.solution.cookies[0].name, "cf_clearance");
        assert_eq!(parsed.solution.cookies[0].value, "abc123");
        assert_eq!(parsed.solution.cookies[0].domain, ".example.com");
        assert!(parsed.solution.cookies[0].httpOnly);
        assert!(parsed.solution.cookies[0].secure);
        assert_eq!(parsed.solution.cookies[0].sameSite, "None");
        assert!(parsed.solution.response.contains("solved page"));
    }

    #[test]
    fn test_flaresolverr_response_null_expiry() {
        let json_str = r#"{
            "status": "ok",
            "message": "",
            "startTimestamp": 0,
            "endTimestamp": 0,
            "version": "3.3.21",
            "solution": {
                "url": "https://example.com",
                "status": 200,
                "cookies": [
                    {
                        "domain": ".example.com",
                        "expiry": null,
                        "httpOnly": false,
                        "name": "session",
                        "path": "/",
                        "sameSite": "Lax",
                        "secure": false,
                        "value": "xyz"
                    }
                ],
                "userAgent": "UA",
                "headers": {},
                "response": ""
            }
        }"#;

        let parsed: FlareSolverrResponse = serde_json::from_str(json_str).unwrap();
        assert!(parsed.solution.cookies[0].expiry.is_none());
        assert!(!parsed.solution.cookies[0].httpOnly);
        assert!(!parsed.solution.cookies[0].secure);
    }

    #[test]
    fn test_flaresolverr_response_multiple_cookies() {
        let json_str = r#"{
            "status": "ok",
            "message": "",
            "startTimestamp": 0,
            "endTimestamp": 0,
            "version": "3.3.21",
            "solution": {
                "url": "https://example.com",
                "status": 200,
                "cookies": [
                    {"domain":".example.com","expiry":null,"httpOnly":false,"name":"a","path":"/","sameSite":"","secure":false,"value":"1"},
                    {"domain":".example.com","expiry":null,"httpOnly":true,"name":"b","path":"/sub","sameSite":"Strict","secure":true,"value":"2"},
                    {"domain":"other.com","expiry":1800000000,"httpOnly":false,"name":"c","path":"/","sameSite":"None","secure":false,"value":"3"}
                ],
                "userAgent": "UA",
                "headers": {"Accept": "text/html"},
                "response": "body"
            }
        }"#;

        let parsed: FlareSolverrResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed.solution.cookies.len(), 3);
        assert_eq!(parsed.solution.cookies[0].name, "a");
        assert_eq!(parsed.solution.cookies[1].name, "b");
        assert_eq!(parsed.solution.cookies[1].path, "/sub");
        assert_eq!(parsed.solution.cookies[1].sameSite, "Strict");
        assert_eq!(parsed.solution.cookies[2].domain, "other.com");
        assert_eq!(parsed.solution.cookies[2].expiry, Some(1800000000));
    }

    #[test]
    fn test_solver_response_outcomes() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        // Synthetic wire responses: no website, browser, or environment changes.
        let solution = |status, body: &str| {
            json!({
                "status": "ok", "message": "", "startTimestamp": 0,
                "endTimestamp": 0, "version": "3.5.2",
                "solution": {"url": "https://example.test/final", "status": status,
                    "cookies": [], "userAgent": "TestUA", "headers": {}, "response": body}
            })
        };
        let cases = [
            (200, solution(200, "content"), None),
            (200, solution(404, "not found"), Some("HTTP 404")),
            (200, solution(503, "unavailable"), Some("HTTP 503")),
            (
                200,
                solution(200, "Cloudflare cf_chl_opt"),
                Some("challenge"),
            ),
            (
                200,
                json!({"status": "error", "message": "solver timeout"}),
                Some("solver timeout"),
            ),
            (
                500,
                json!({"status": "error", "message": "The session doesn't exist."}),
                Some("The session doesn't exist."),
            ),
            (
                200,
                json!({"status": "ok", "message": "", "solution": null}),
                Some("solution"),
            ),
            (500, solution(200, "content"), Some("HTTP 500")),
        ];
        for (http_status, envelope, expected_error) in cases {
            for method in ["GET", "POST form", "POST empty"] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                listener.set_nonblocking(true).unwrap();
                let endpoint = format!("http://{}", listener.local_addr().unwrap());
                let body = envelope.to_string();
                let worker = std::thread::spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(Instant::now() < deadline, "solver stub accept timed out");
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            Err(e) => panic!("solver stub accept failed: {e}"),
                        }
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut byte = [0];
                    while !request.ends_with(b"\r\n\r\n") {
                        stream.read_exact(&mut byte).unwrap();
                        request.push(byte[0]);
                        assert!(request.len() < 16384, "oversized request headers");
                    }
                    let headers = String::from_utf8(request).unwrap();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    assert!(length < 16384);
                    let mut payload = vec![0; length];
                    stream.read_exact(&mut payload).unwrap();
                    let payload: JsonValue = serde_json::from_slice(&payload).unwrap();
                    write!(stream, "HTTP/1.1 {http_status} Stub\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    payload
                });
                let result = match method {
                    "GET" => {
                        proxy_fetch_text(&endpoint, Some("test-session"), "https://example.test")
                    }
                    "POST form" => proxy_post_form(
                        &endpoint,
                        Some("test-session"),
                        "https://example.test",
                        &[("q", "a&b")],
                    ),
                    _ => proxy_post_empty(&endpoint, Some("test-session"), "https://example.test"),
                };
                let payload = worker.join().unwrap();
                assert_eq!(payload["session"], "test-session");
                assert_eq!(payload["url"], "https://example.test");
                assert_eq!(
                    payload["cmd"],
                    if method == "GET" {
                        "request.get"
                    } else {
                        "request.post"
                    }
                );
                if method == "POST form" {
                    assert_eq!(payload["postData"], "q=a%26b");
                }
                if method == "POST empty" {
                    assert_eq!(payload["postData"], "");
                }
                match expected_error {
                    Some(expected) => {
                        let error = result.expect_err("solver failure must not become content");
                        assert!(
                            format!("{error:#}").contains(expected),
                            "{method}: {error:#}"
                        );
                        assert_eq!(
                            is_missing_flaresolverr_session(&error),
                            expected == "The session doesn't exist."
                        );
                    }
                    None => assert_eq!(result.unwrap(), "content"),
                }
            }
        }
    }

    // --- insert_flaresolverr_cookies_into_agent ----------------------------

    #[test]
    fn test_insert_cookies_into_agent() {
        let agent = build_ureq_agent(Some("TestUA"));
        let cookies = vec![
            mock_cookie("cf_clearance", "test_value", ".example.com"),
            mock_cookie("session_id", "sess_abc", ".example.com"),
        ];

        // Should not panic — verifies the full cookie parsing + insertion pipeline
        insert_flaresolverr_cookies_into_agent(&agent, cookies);

        // Insert again with different values to verify overwrite doesn't panic
        let cookies2 = vec![mock_cookie("cf_clearance", "new_value", ".example.com")];
        insert_flaresolverr_cookies_into_agent(&agent, cookies2);
    }

    #[test]
    fn test_insert_cookies_domain_with_https_prefix() {
        let agent = build_ureq_agent(None);
        let cookies = vec![FlareSolverrCookie {
            domain: "https://cdn.example.com".to_string(),
            expiry: None,
            httpOnly: false,
            name: "token".to_string(),
            path: "/".to_string(),
            sameSite: "".to_string(),
            secure: false,
            value: "abc".to_string(),
        }];

        // Should not panic even with an https:// prefixed domain
        insert_flaresolverr_cookies_into_agent(&agent, cookies);
    }

    #[test]
    fn test_insert_cookies_empty_domain_fallback() {
        let agent = build_ureq_agent(None);
        let cookies = vec![FlareSolverrCookie {
            domain: "".to_string(),
            expiry: None,
            httpOnly: false,
            name: "x".to_string(),
            path: "/".to_string(),
            sameSite: "".to_string(),
            secure: false,
            value: "y".to_string(),
        }];

        // Should not panic — falls back to https://example.com
        insert_flaresolverr_cookies_into_agent(&agent, cookies);
    }

    // --- build_ureq_agent --------------------------------------------------

    #[test]
    fn test_build_ureq_agent_with_ua() {
        let agent = build_ureq_agent(Some("CustomUA/1.0"));
        // Agent is created without panic; UA is embedded in config.
        let _ = agent;
    }

    #[test]
    fn test_build_ureq_agent_no_ua() {
        let agent = build_ureq_agent(None);
        let _ = agent;
    }

    #[test]
    fn test_build_ureq_agent_empty_ua() {
        // Empty string should be skipped, not set.
        let agent = build_ureq_agent(Some(""));
        let _ = agent;
    }

    // --- FlareClient plain -------------------------------------------------

    #[test]
    fn test_plain_client_no_flaresolverr() {
        let client = FlareClient::plain();
        let guard = client.inner.lock().unwrap();
        assert!(guard.flaresolverr_url.is_none());
        assert!(guard.session_id.is_none());
        assert!(guard.default_headers.is_empty());
        assert!(guard.origin_url.is_empty());
        assert!(guard.limiter.is_none());
        assert!(guard.direct_works);
    }

    #[test]
    fn test_direct_works_starts_true() {
        let client = FlareClient::plain_with_rps(Some(1.0));
        let guard = client.inner.lock().unwrap();
        assert!(guard.direct_works, "direct_works should start as true");
    }

    #[test]
    fn test_direct_path_reenables_after_cooldown() {
        let client = FlareClient::plain();
        {
            let mut guard = client.inner.lock().unwrap();
            guard.direct_works = false;
            guard.direct_disabled_until = Some(Instant::now() - Duration::from_secs(1));
        }

        let (should_try_direct, has_fs) = client.direct_path_state();

        assert!(should_try_direct);
        assert!(!has_fs);
        let guard = client.inner.lock().unwrap();
        assert!(guard.direct_works);
        assert!(guard.direct_disabled_until.is_none());
    }

    #[test]
    fn test_plain_client_with_rps() {
        let client = FlareClient::plain_with_rps(Some(2.0));
        let guard = client.inner.lock().unwrap();
        assert!(guard.flaresolverr_url.is_none());
        assert!(guard.limiter.is_some());
    }

    #[test]
    fn test_plain_client_with_zero_rps() {
        // Zero or negative RPS should result in no limiter
        let client = FlareClient::plain_with_rps(Some(0.0));
        let guard = client.inner.lock().unwrap();
        assert!(guard.limiter.is_none());
    }

    #[test]
    fn test_plain_client_with_negative_rps() {
        let client = FlareClient::plain_with_rps(Some(-5.0));
        let guard = client.inner.lock().unwrap();
        assert!(guard.limiter.is_none());
    }

    #[test]
    fn test_plain_client_with_nan_rps() {
        let client = FlareClient::plain_with_rps(Some(f64::NAN));
        let guard = client.inner.lock().unwrap();
        assert!(guard.limiter.is_none());
    }

    #[test]
    fn test_plain_client_with_infinity_rps() {
        let client = FlareClient::plain_with_rps(Some(f64::INFINITY));
        let guard = client.inner.lock().unwrap();
        assert!(guard.limiter.is_none());
    }

    // --- FlareClient::from_env without FLARESOLVERR_URL set ----------------

    #[test]
    fn test_from_env_no_env_var_is_plain() {
        let _guard = env_test_guard();
        // Ensure env var is not set for this test
        // SAFETY: Test-only; single-threaded test runner for env-dependent tests.
        unsafe { env::remove_var("FLARESOLVERR_URL") };
        let client = FlareClient::from_env("https://example.com").unwrap();
        let guard = client.inner.lock().unwrap();
        assert!(guard.flaresolverr_url.is_none());
        assert_eq!(guard.origin_url, "https://example.com");
    }

    #[test]
    fn test_from_env_or_plain_no_env_var() {
        let _guard = env_test_guard();
        unsafe { env::remove_var("FLARESOLVERR_URL") };
        let client = FlareClient::from_env_or_plain("https://example.com");
        let guard = client.inner.lock().unwrap();
        assert!(guard.flaresolverr_url.is_none());
    }

    #[test]
    fn test_named_session_is_lazy_and_explicit_override_wins() {
        let _guard = env_test_guard();
        let previous_url = env::var("FLARESOLVERR_URL").ok();
        let previous_session = env::var("FLARESOLVERR_SESSION").ok();

        unsafe {
            env::set_var("FLARESOLVERR_URL", "http://127.0.0.1:8191/v1");
            env::remove_var("FLARESOLVERR_SESSION");
        }

        let client = FlareClient::from_env_with_rps_and_session(
            "https://example.com",
            None,
            Some("tanoshi-example"),
        )
        .unwrap();
        let guard = client.inner.lock().unwrap();
        assert!(guard.session_id.is_none());
        assert_eq!(guard.session_name.as_deref(), Some("tanoshi-example"));
        drop(guard);

        unsafe { env::set_var("FLARESOLVERR_SESSION", "user-supplied") };
        let overridden = FlareClient::from_env_with_rps_and_session(
            "https://example.com",
            None,
            Some("tanoshi-example"),
        )
        .unwrap();
        let guard = overridden.inner.lock().unwrap();
        assert_eq!(guard.session_id.as_deref(), Some("user-supplied"));
        assert!(guard.session_name.is_none());
        drop(guard);

        unsafe {
            match previous_url {
                Some(value) => env::set_var("FLARESOLVERR_URL", value),
                None => env::remove_var("FLARESOLVERR_URL"),
            }
            match previous_session {
                Some(value) => env::set_var("FLARESOLVERR_SESSION", value),
                None => env::remove_var("FLARESOLVERR_SESSION"),
            }
        }
    }

    // --- FlareClient::re_solve without FS ----------------------------------

    #[test]
    fn test_re_solve_returns_false_without_flaresolverr() {
        let client = FlareClient::plain();
        let result = client.re_solve().unwrap();
        assert!(
            !result,
            "re_solve should return Ok(false) when no FS configured"
        );
    }

    // --- HTTP status semantics ---------------------------------------------

    /// Serve one HTTP response on a local port, return the URL to request.
    fn serve_once(response: &'static str) -> String {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{}", addr)
    }

    const NOT_FOUND_RESPONSE: &str =
        "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found";

    #[test]
    fn test_rate_limited_agent_errors_on_http_status() {
        // RateLimitedAgent keeps ureq's status-as-error default: callers
        // parse response bodies unconditionally, so a 404 page must surface
        // as Err instead of being parsed into empty results.
        let url = serve_once(NOT_FOUND_RESPONSE);
        let agent = build_rate_limited_ureq_agent(None, None);
        assert!(
            agent.get(&url).call().is_err(),
            "non-2xx must be an error for RateLimitedAgent"
        );
    }

    #[test]
    fn test_flare_client_classifies_http_error() {
        // FlareClient's lenient agent must receive non-2xx bodies so
        // challenge classification can inspect them.
        let url = serve_once(NOT_FOUND_RESPONSE);
        let client = FlareClient::plain();
        match client.try_direct_get(&url).unwrap() {
            DirectResult::HttpError(status) => assert_eq!(status, 404),
            DirectResult::Success(_) => panic!("404 should classify as HttpError, got Success"),
            DirectResult::Challenged(_) => {
                panic!("404 should classify as HttpError, got Challenged")
            }
        }
    }

    #[test]
    fn test_fetch_text_propagates_http_error() {
        // Without FlareSolverr there is nothing to escalate to: a plain 404
        // must come back as an error naming the status, not a silent body.
        let url = serve_once(NOT_FOUND_RESPONSE);
        let client = FlareClient::plain();
        let err = client.fetch_text(&url).unwrap_err();
        assert!(
            err.to_string().contains("404"),
            "error should carry the HTTP status: {err:#}"
        );
    }

    #[test]
    fn test_should_escalate_status() {
        assert!(should_escalate_status(403));
        assert!(should_escalate_status(429));
        assert!(should_escalate_status(503));
        assert!(!should_escalate_status(404));
        assert!(!should_escalate_status(500));
        assert!(!should_escalate_status(200));
    }

    // --- RateLimitedAgent --------------------------------------------------

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

    // --- RateLimiter -------------------------------------------------------

    #[test]
    fn test_rate_limiter_valid_rps() {
        let limiter = RateLimiter::new(10.0);
        assert!(limiter.is_some());
    }

    #[test]
    fn test_rate_limiter_zero_rps() {
        assert!(RateLimiter::new(0.0).is_none());
    }

    #[test]
    fn test_rate_limiter_negative_rps() {
        assert!(RateLimiter::new(-1.0).is_none());
    }

    #[test]
    fn test_rate_limiter_nan() {
        assert!(RateLimiter::new(f64::NAN).is_none());
    }

    #[test]
    fn test_rate_limiter_infinity() {
        assert!(RateLimiter::new(f64::INFINITY).is_none());
    }

    #[test]
    fn test_rate_limiter_acquire_does_not_block_first_call() {
        let limiter = RateLimiter::new(1000.0).unwrap(); // high RPS
        let start = std::time::Instant::now();
        limiter.acquire();
        let elapsed = start.elapsed();
        // First acquire should be near-instant
        assert!(
            elapsed.as_millis() < 50,
            "First acquire took too long: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_rate_limiter_enforces_interval() {
        // 10 RPS = 100ms between requests
        let limiter = RateLimiter::new(10.0).unwrap();
        limiter.acquire(); // first: instant
        let start = std::time::Instant::now();
        limiter.acquire(); // second: should wait ~100ms
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() >= 80, // some tolerance
            "Second acquire should have waited ~100ms, took {:?}",
            elapsed
        );
    }

    // --- build_image_get ---------------------------------------------------

    #[test]
    fn test_build_image_get_sets_accept_header() {
        let agent = build_ureq_agent(None);
        let req = agent.get("https://cdn.example.com/image.png");
        let req = build_image_get("https://cdn.example.com/image.png", req);
        // We can't easily inspect headers on the builder, but we verify
        // it doesn't panic and the request can be built.
        let _ = req;
    }

    // --- DirectResult via try_direct_get -----------------------------------

    #[test]
    fn test_direct_result_enum_variants() {
        // Ensure the enum is constructable (compile-time check mostly)
        let success = DirectResult::Success("hello".to_string());
        let challenged = DirectResult::Challenged(403);
        let http_error = DirectResult::HttpError(404);

        match success {
            DirectResult::Success(s) => assert_eq!(s, "hello"),
            _ => panic!("Expected Success"),
        }

        match challenged {
            DirectResult::Challenged(code) => assert_eq!(code, 403),
            _ => panic!("Expected Challenged"),
        }

        match http_error {
            DirectResult::HttpError(code) => assert_eq!(code, 404),
            _ => panic!("Expected HttpError"),
        }
    }

    // --- build_rate_limited_flaresolverr_client without env -----------------

    #[test]
    fn test_build_rate_limited_flaresolverr_client_no_env() {
        let _guard = env_test_guard();
        unsafe { env::remove_var("FLARESOLVERR_URL") };
        // Should fall back to plain client without panicking
        let client = build_rate_limited_flaresolverr_client("https://example.com", Some(3.0));
        let guard = client.inner.lock().unwrap();
        assert!(guard.flaresolverr_url.is_none());
        assert!(guard.limiter.is_some());
    }

    // --- FlareClient is Clone + Send + Sync --------------------------------

    #[test]
    fn test_flare_client_is_clone_send_sync() {
        fn assert_send_sync<T: Send + Sync + Clone>() {}
        assert_send_sync::<FlareClient>();
    }

    #[test]
    fn test_rate_limited_agent_is_clone_send_sync() {
        fn assert_send_sync<T: Send + Sync + Clone>() {}
        assert_send_sync::<RateLimitedAgent>();
    }

    // --- Thread safety: concurrent access ----------------------------------

    #[test]
    fn test_flare_client_concurrent_re_solve_no_panic() {
        // Without FS configured, re_solve returns Ok(false).
        // Verify no deadlocks under concurrent access.
        let client = FlareClient::plain();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let c = client.clone();
                std::thread::spawn(move || {
                    let result = c.re_solve().unwrap();
                    assert!(!result);
                })
            })
            .collect();

        for h in handles {
            h.join().expect("Thread panicked");
        }
    }

    // =======================================================================
    // Integration tests — require running FlareSolverr + network access
    // Run with: cargo test -- --ignored
    // =======================================================================

    #[test]
    #[ignore = "live source check"]
    fn test_nowsecure() {
        let _guard = env_test_guard();
        let fs_url = flaresolverr_url();

        let flare_body = get_flaresolverr_response("https://nowsecure.com", &fs_url);
        assert_eq!(flare_body.status, "ok");
        assert!(!flare_body.solution.response.is_empty());
        assert!(!flare_body.solution.userAgent.is_empty());
        assert!(!flare_body.solution.cookies.is_empty());

        let ureq_body = get_ureq_response("https://nowsecure.com", &fs_url);
        assert!(!ureq_body.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_openai() {
        let _guard = env_test_guard();
        let fs_url = flaresolverr_url();

        let flare_body = get_flaresolverr_response("https://openai.com", &fs_url);
        assert_eq!(flare_body.status, "ok");
        assert!(!flare_body.solution.response.is_empty());

        let ureq_body = get_ureq_response("https://openai.com", &fs_url);
        assert!(!ureq_body.is_empty());
    }

    /// Integration: FlareClient direct-first strategy against a CF-protected site.
    /// Validates that:
    ///   1. construction does not perform a solve
    ///   2. fetch_text succeeds via the direct path or lazy solve
    ///   3. The returned HTML is the real page, not a challenge
    #[test]
    #[ignore = "live source check"]
    fn test_flare_client_direct_first_fetch() {
        let _guard = env_test_guard();
        let fs_url = flaresolverr_url();
        unsafe { env::set_var("FLARESOLVERR_URL", &fs_url) };

        let client = FlareClient::from_env("https://nowsecure.com").unwrap();

        // Verify construction only stored the configured request state.
        {
            let guard = client.inner.lock().unwrap();
            assert!(guard.flaresolverr_url.is_some());
            assert_eq!(guard.origin_url, "https://nowsecure.com");
            // No solve should have happened during construction.
            assert!(guard.default_headers.is_empty());
        }

        let body = client.fetch_text("https://nowsecure.com").unwrap();
        assert!(!body.is_empty());
        // The body should NOT be a challenge page
        assert!(
            !looks_like_cf_challenge(200, &body),
            "fetch_text returned a challenge page instead of the real content"
        );
    }

    /// Integration: FlareClient.fetch_bytes for image fetching
    #[test]
    #[ignore = "live source check"]
    fn test_flare_client_fetch_bytes() {
        // Use a known public image URL (not CF-protected, just validates
        // the fetch_bytes pipeline works end-to-end).
        let client = FlareClient::plain();
        let bytes = client.fetch_bytes("https://httpbin.org/image/png").unwrap();
        assert!(!bytes.is_empty());
        // PNG magic bytes
        assert_eq!(&bytes[0..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    /// Integration: RateLimitedAgent.fetch_bytes
    #[test]
    #[ignore = "live source check"]
    fn test_rate_limited_agent_fetch_bytes() {
        let agent = build_rate_limited_ureq_agent(None, Some(5.0));
        let bytes = agent
            .fetch_bytes("https://httpbin.org/image/png", None)
            .unwrap();
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[0..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    /// Integration: solve_with_flaresolverr returns proper Solved struct
    #[test]
    #[ignore = "live source check"]
    fn test_solve_with_flaresolverr_struct() {
        let _guard = env_test_guard();
        let fs_url = flaresolverr_url();
        let solved = solve_with_flaresolverr(&fs_url, "https://nowsecure.com", None).unwrap();

        assert!(
            !solved.user_agent.is_empty(),
            "user_agent should not be empty"
        );
        assert!(!solved.cookies.is_empty(), "should have received cookies");

        // At least one cookie should be cf_clearance
        let has_clearance = solved.cookies.iter().any(|c| c.name == "cf_clearance");
        assert!(
            has_clearance,
            "Expected cf_clearance cookie in solved cookies: {:?}",
            solved.cookies.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    /// Integration: FlareClient.post_form_text with direct-first strategy
    #[test]
    #[ignore = "live source check"]
    fn test_flare_client_post_form() {
        // httpbin echoes back form data — validates the POST pipeline
        let client = FlareClient::plain();
        let body = client
            .post_form_text(
                "https://httpbin.org/post",
                &[("key", "value"), ("foo", "bar")],
            )
            .unwrap();

        assert!(body.contains("key"));
        assert!(body.contains("value"));
        assert!(body.contains("foo"));
        assert!(body.contains("bar"));
    }

    /// Integration: FlareClient.post_empty_text
    #[test]
    #[ignore = "live source check"]
    fn test_flare_client_post_empty() {
        let client = FlareClient::plain();
        let body = client
            .post_empty_text("https://httpbin.org/post", &[("X-Custom", "hello")])
            .unwrap();

        assert!(!body.is_empty());
        assert!(body.contains("X-Custom"));
    }

    /// Integration: FlareClient does not create a session by default
    #[test]
    #[ignore = "live source check"]
    fn test_flare_client_session_not_created_by_default() {
        let _guard = env_test_guard();
        let fs_url = flaresolverr_url();
        unsafe { env::set_var("FLARESOLVERR_URL", &fs_url) };
        unsafe { env::remove_var("FLARESOLVERR_SESSION") };

        let client = FlareClient::from_env("https://nowsecure.com").unwrap();
        let guard = client.inner.lock().unwrap();

        assert!(guard.session_id.is_none());
    }

    /// Integration: multiple sequential fetches reuse the same agent (direct path)
    #[test]
    #[ignore = "live source check"]
    fn test_flare_client_multiple_fetches_reuse_agent() {
        let _guard = env_test_guard();
        let fs_url = flaresolverr_url();
        unsafe { env::set_var("FLARESOLVERR_URL", &fs_url) };

        let client = FlareClient::from_env("https://nowsecure.com").unwrap();

        // Fetch the same URL multiple times — all should succeed via direct path
        for i in 0..3 {
            let body = client.fetch_text("https://nowsecure.com").unwrap();
            assert!(!body.is_empty(), "Fetch #{} returned empty body", i + 1);
            assert!(
                !looks_like_cf_challenge(200, &body),
                "Fetch #{} returned a challenge page",
                i + 1
            );
        }
    }
}
