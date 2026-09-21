use super::protocol::{FlareSolverrCookie, FlareSolverrSolution, request_flaresolverr};
use crate::client::build_ureq_agent;
#[allow(non_snake_case)]
#[derive(Debug, serde::Deserialize, Clone)]
struct FlareSolverrResponse {
    pub status: String,
    pub message: String,
    pub solution: FlareSolverrSolution,
    #[serde(rename = "startTimestamp")]
    _start_timestamp: u64,
    #[serde(rename = "endTimestamp")]
    _end_timestamp: u64,
    pub version: String,
}

use super::*;
use crate::client::{RateLimitedAgent, build_rate_limited_ureq_agent};
use cookie::time::OffsetDateTime as CookieOffsetDateTime;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::env;
use std::process::Command;
use url::Url;

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

fn flaresolverr_url() -> String {
    env::var("FLARESOLVERR_URL").unwrap_or_else(|_| "http://localhost:8191/v1".to_string())
}

/// Run adapter assertions in a fresh process whose environment is fixed before
/// its test runner starts. No test mutates the shared process environment.
fn isolated_env_case(
    case: &str,
    solver_url: Option<&str>,
    session: Option<&str>,
    assertions: impl FnOnce(),
) {
    const CHILD_CASE: &str = "TANOSHI_NETWORKING_ENV_TEST_CASE";
    if let Ok(selected) = env::var(CHILD_CASE) {
        if selected == case {
            assertions();
        }
        return;
    }

    let thread = std::thread::current();
    let test_name = thread.name().expect("named test thread");
    let mut command = Command::new(env::current_exe().unwrap());
    command
        .args(["--exact", test_name, "--test-threads=1"])
        .env(CHILD_CASE, case)
        .env_remove("FLARESOLVERR_URL")
        .env_remove("FLARESOLVERR_SESSION");
    if let Some(url) = solver_url {
        command.env("FLARESOLVERR_URL", url);
    }
    if let Some(session) = session {
        command.env("FLARESOLVERR_SESSION", session);
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "isolated case {case} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn get_flaresolverr_response(url: &str, flaresolverr_url: &str) -> FlareSolverrResponse {
    let payload = json!({
        "cmd": "request.get",
        "url": url,
        "maxTimeout": Operation::new(None).request_timeout().unwrap().as_millis() as u64,
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
    // Replay the solver's cookies and user agent in a direct HTTP request.
    let payload = json!({"cmd": "request.get", "url": url});
    let solution = request_flaresolverr(flaresolverr_url, &payload, &Operation::new(None)).unwrap();
    let client = build_ureq_agent(Some(&solution.userAgent));
    insert_flaresolverr_cookies_into_agent(&client, &solution.url, solution.cookies).unwrap();
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
fn test_cf_challenge_detection_200_passive_script_not_flagged() {
    let body = r#"<html><script src="/cdn-cgi/challenge-platform/scripts/jsd/main.js"></script>
        cloudflare</html>"#;
    assert!(!looks_like_cf_challenge(200, body));
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
            "solution": {"url": "https://redirected.example.test/final", "status": status,
                "cookies": [{"domain": "", "expiry": null, "httpOnly": true,
                    "name": "session", "value": "test-value", "path": "/", "sameSite": "Lax", "secure": true}], "userAgent": "TestUA",
                "headers": {"Content-Type": "text/html", "Content-Length": "9000", "X-Response-Only": "do-not-replay"},
                "response": body}
        })
    };
    let cases = [
        (200, solution(200, "content"), None),
        (200, solution(404, "not found"), Some("HTTP 404")),
        (200, solution(503, "unavailable"), Some("HTTP 503")),
        (200, solution(429, "slow down"), Some("HTTP 429")),
        (
            200,
            solution(429, "Cloudflare cf_chl_opt"),
            Some("HTTP 429"),
        ),
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
        for method in ["GET", "POST form", "POST empty", "re-solve"] {
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
                "GET" => proxy_fetch_document(
                    &endpoint,
                    Some("test-session"),
                    "https://example.test",
                    &Operation::new(None),
                ),
                "POST form" => proxy_post_form(
                    &endpoint,
                    Some("test-session"),
                    "https://example.test",
                    &[("q", "a&b")],
                    &Operation::new(None),
                ),
                "POST empty" => proxy_post_empty(
                    &endpoint,
                    Some("test-session"),
                    "https://example.test",
                    &Operation::new(None),
                ),
                _ => {
                    let client = FlareClient::new(FlareClientConfig::default());
                    {
                        let mut inner = client.lock_inner();
                        inner.flaresolverr_url = Some(endpoint.clone());
                        inner.origin_url = "https://example.test".to_string();
                        inner.session = FlareSession::External("test-session".to_string()).into();
                    }
                    client
                        .re_solve(&client.pending_solve(), &client.operation())
                        .and_then(|solved| {
                            if !solved || !client.lock_inner().default_headers.is_empty() {
                                return Err(anyhow!(
                                    "response headers must not become request defaults"
                                ));
                            }
                            if client
                                .lock_inner()
                                .agent
                                .cookie_jar_lock()
                                .get("redirected.example.test", "/", "session")
                                .is_none()
                            {
                                return Err(anyhow!("cookie must use the final solved URL"));
                            }
                            Ok(FetchedDocument {
                                final_url: "https://redirected.example.test/final".into(),
                                body: "content".into(),
                            })
                        })
                }
            };
            let payload = worker.join().unwrap();
            assert_eq!(payload["session"], "test-session");
            assert_eq!(payload["url"], "https://example.test");
            assert_eq!(
                payload["cmd"],
                if method == "GET" || method == "re-solve" {
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
                    assert_eq!(
                        solve::error_is::<crate::backoff::RateLimited>(&error),
                        expected == "HTTP 429"
                    );
                }
                None => {
                    let document = result.unwrap();
                    assert_eq!(document.body, "content");
                    assert_eq!(document.final_url, "https://redirected.example.test/final");
                }
            }
        }
    }
}

// --- insert_flaresolverr_cookies_into_agent ----------------------------

#[test]
fn test_insert_cookies_into_agent() {
    let agent = build_ureq_agent(Some("TestUA"));
    let origin = "https://reader.example.com/chapters/1";
    let mut scoped = mock_cookie("scoped", "path", ".example.com");
    scoped.path = "/chapters".to_string();
    let mut expired = mock_cookie("expired", "must-not-send", ".example.com");
    expired.expiry = Some(1);
    let mut invalid_expiry = expired.clone();
    invalid_expiry.name = "invalid_expiry".to_string();
    invalid_expiry.expiry = Some(u64::MAX);
    let cookies = vec![
        mock_cookie("domain", "old", ".example.com"),
        mock_cookie("host", "host-only", "reader.example.com"),
        scoped,
        expired,
        invalid_expiry,
        mock_cookie("suffix", "must-not-send", ".com"),
        mock_cookie("foreign", "must-not-send", ".other.com"),
        mock_cookie("foreign_host", "must-not-send", "other.com"),
    ];
    insert_flaresolverr_cookies_into_agent(&agent, origin, cookies).unwrap();
    insert_flaresolverr_cookies_into_agent(
        &agent,
        origin,
        vec![mock_cookie("domain", "new", ".EXAMPLE.COM")],
    )
    .unwrap();
    assert_eq!(agent.cookie_jar_lock().iter().count(), 3);

    // Inspect the jar's actual stored scope through cookie_store's request
    // matching API, the same API ureq uses to build its Cookie header.
    let mut saved = Vec::new();
    agent.cookie_jar_lock().save_json(&mut saved).unwrap();
    let store = cookie_store::serde::json::load(saved.as_slice()).unwrap();
    for (url, expected) in [
        (
            origin,
            vec![("domain", "new"), ("host", "host-only"), ("scoped", "path")],
        ),
        (
            "https://cdn.example.com/chapters/2",
            vec![("domain", "new"), ("scoped", "path")],
        ),
        ("https://child.reader.example.com/", vec![("domain", "new")]),
        (
            "https://reader.example.com/chapters-extra",
            vec![("domain", "new"), ("host", "host-only")],
        ),
        ("http://reader.example.com/chapters/1", vec![]),
        ("https://other.com/chapters/1", vec![]),
    ] {
        let mut actual: Vec<_> = store
            .get_request_values(&Url::parse(url).unwrap())
            .collect();
        actual.sort_unstable();
        assert_eq!(actual, expected, "cookie selection for {url}");
    }
}

#[test]
fn test_insert_cookies_rejects_invalid_and_public_suffix_domains() {
    for (origin, domain) in [
        ("https://cdn.example.com", "https://cdn.example.com"),
        ("https://example.co.uk", ".co.uk"),
        ("https://tenant.github.io", ".github.io"),
        ("https://foo.ck", ".ck"),
        ("https://tenant.foo.ck", ".foo.ck"),
    ] {
        let agent = build_ureq_agent(None);
        insert_flaresolverr_cookies_into_agent(
            &agent,
            origin,
            vec![mock_cookie("token", "must-not-send", domain)],
        )
        .unwrap();
        assert_eq!(
            agent.cookie_jar_lock().iter().count(),
            0,
            "accepted {domain}"
        );
    }
    // Public-suffix exception rules must still permit registrable domains.
    let agent = build_ureq_agent(None);
    insert_flaresolverr_cookies_into_agent(
        &agent,
        "https://www.ck/",
        vec![mock_cookie("token", "allowed", ".www.ck")],
    )
    .unwrap();
    assert_eq!(agent.cookie_jar_lock().iter().count(), 1);
}

#[test]
fn test_insert_cookies_empty_domain_uses_solved_origin() {
    let agent = build_ureq_agent(None);
    let mut cookie = mock_cookie("session", "value", "");
    cookie.expiry = None;
    cookie.path.clear();
    insert_flaresolverr_cookies_into_agent(
        &agent,
        "https://redirected.example.org/reader/chapter",
        vec![cookie],
    )
    .unwrap();
    let jar = agent.cookie_jar_lock();
    assert_eq!(jar.iter().count(), 1);
    assert_eq!(
        jar.get("redirected.example.org", "/reader", "session")
            .unwrap()
            .value(),
        "value"
    );
    assert!(jar.get("example.com", "/", "session").is_none());
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
    let client = FlareClient::new(FlareClientConfig::default());
    let guard = client.inner.lock().unwrap();
    assert!(guard.flaresolverr_url.is_none());
    assert!(guard.session.current().is_none());
    assert!(guard.default_headers.is_empty());
    assert!(guard.origin_url.is_empty());
    assert!(guard.limiter.is_none());
    assert!(matches!(guard.route, RouteState::DirectEligible));
}

#[test]
fn test_direct_works_starts_true() {
    let client = FlareClient::new(FlareClientConfig {
        requests_per_second: Some(1.0),
        ..Default::default()
    });
    let guard = client.inner.lock().unwrap();
    assert!(matches!(guard.route, RouteState::DirectEligible));
}

#[test]
fn test_direct_path_reenables_after_cooldown() {
    let client = FlareClient::new(FlareClientConfig::default());
    {
        let mut guard = client.inner.lock().unwrap();
        guard.route = RouteState::ProxyUntil(Instant::now() - Duration::from_secs(1));
    }

    let (should_try_direct, has_fs) = client.direct_path_state();

    assert!(should_try_direct);
    assert!(!has_fs);
    let guard = client.inner.lock().unwrap();
    assert!(matches!(guard.route, RouteState::DirectEligible));
}

#[test]
fn test_plain_client_with_rps() {
    let client = FlareClient::new(FlareClientConfig {
        requests_per_second: Some(2.0),
        ..Default::default()
    });
    let guard = client.inner.lock().unwrap();
    assert!(guard.flaresolverr_url.is_none());
    assert!(guard.limiter.is_some());
}

#[test]
fn test_plain_client_with_zero_rps() {
    // Invalid explicit rates must fail on use, not silently disable pacing.
    let client = FlareClient::new(FlareClientConfig {
        requests_per_second: Some(0.0),
        ..Default::default()
    });
    let error = client.fetch_text("http://127.0.0.1:1/").unwrap_err();
    assert!(error.to_string().contains("requests_per_second"));
}

#[test]
fn test_plain_client_with_negative_rps() {
    let client = FlareClient::new(FlareClientConfig {
        requests_per_second: Some(-5.0),
        ..Default::default()
    });
    let error = client.fetch_text("http://127.0.0.1:1/").unwrap_err();
    assert!(error.to_string().contains("requests_per_second"));
}

#[test]
fn test_plain_client_with_nan_rps() {
    let client = FlareClient::new(FlareClientConfig {
        requests_per_second: Some(f64::NAN),
        ..Default::default()
    });
    let error = client.fetch_text("http://127.0.0.1:1/").unwrap_err();
    assert!(error.to_string().contains("requests_per_second"));
}

#[test]
fn test_plain_client_with_infinity_rps() {
    let client = FlareClient::new(FlareClientConfig {
        requests_per_second: Some(f64::INFINITY),
        ..Default::default()
    });
    let error = client.fetch_text("http://127.0.0.1:1/").unwrap_err();
    assert!(error.to_string().contains("requests_per_second"));
}

// --- Environment adapter without FLARESOLVERR_URL set ----------------

#[test]
fn test_from_env_no_env_var_is_plain() {
    isolated_env_case("absent", None, None, || {
        let client = FlareClient::new(FlareClientConfig::from_env(
            "https://example.com",
            None,
            None,
        ));
        let guard = client.inner.lock().unwrap();
        assert!(guard.flaresolverr_url.is_none());
        assert_eq!(guard.origin_url, "https://example.com");
    });
}

#[test]
fn test_explicit_config_without_solver() {
    let client = FlareClient::new(FlareClientConfig {
        origin_url: "https://example.com".to_string(),
        ..Default::default()
    });
    let guard = client.inner.lock().unwrap();
    assert!(guard.flaresolverr_url.is_none());
}

#[test]
fn test_named_session_is_lazy_and_explicit_override_wins() {
    for (case, session) in [("managed", None), ("external", Some("user-supplied"))] {
        isolated_env_case(case, Some("http://127.0.0.1:8191/v1"), session, || {
            let client = FlareClient::new(FlareClientConfig::from_env(
                "https://example.com",
                None,
                Some("tanoshi-example"),
            ));
            let guard = client.inner.lock().unwrap();
            assert_eq!(
                guard.session.current().as_ref().map(|s| s.id.as_str()),
                session
            );
            assert_eq!(
                guard.session.managed_name(),
                if session.is_some() {
                    None
                } else {
                    Some("tanoshi-example")
                }
            );
        });
    }
}

// --- FlareClient::re_solve without FS ----------------------------------

#[test]
fn test_re_solve_returns_false_without_flaresolverr() {
    let client = FlareClient::new(FlareClientConfig::default());
    let result = client
        .re_solve(&client.pending_solve(), &client.operation())
        .unwrap();
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

#[test]
fn test_documents_retain_redirect_url() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    for method in ["plain GET", "flare GET", "POST form", "POST empty"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for redirect in [true, false] {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "redirect request timed out");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut headers = Vec::new();
                let mut byte = [0];
                while !headers.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    headers.push(byte[0]);
                    assert!(headers.len() < 16384);
                }
                let headers = String::from_utf8(headers).unwrap();
                if redirect {
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    assert!(length < 16384);
                    let mut body = vec![0; length];
                    stream.read_exact(&mut body).unwrap();
                    assert!(headers.starts_with(if method.starts_with("POST") {
                        "POST /old HTTP/1.1"
                    } else {
                        "GET /old HTTP/1.1"
                    }));
                    assert_eq!(
                        body,
                        if method == "POST form" {
                            &b"q=a%26b"[..]
                        } else {
                            &b""[..]
                        }
                    );
                    stream.write_all(b"HTTP/1.1 303 See Other\r\nLocation: /new/chapter?lang=en\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                } else {
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\ncontent").unwrap();
                    assert!(headers.starts_with("GET /new/chapter?lang=en HTTP/1.1"));
                    for name in ["content-length:", "transfer-encoding:"] {
                        assert!(
                            !headers
                                .lines()
                                .any(|line| line.to_ascii_lowercase().starts_with(name)),
                            "{method}: body header retained on redirected GET: {name}"
                        );
                    }
                }
            }
        });
        let url = format!("{base}/old");
        let client = FlareClient::new(FlareClientConfig::default());
        let document = match method {
            "plain GET" => build_rate_limited_ureq_agent(None, None).fetch_document(&url),
            "POST form" => client.post_form_document(&url, &[("q", "a&b")]),
            "POST empty" => {
                client.post_empty_document(&url, &[("Content-Type", "application/test")])
            }
            _ => client.fetch_document(&url),
        }
        .unwrap();
        worker.join().unwrap();
        assert_eq!(document.body, "content", "{method}");
        assert_eq!(
            document.final_url,
            format!("{base}/new/chapter?lang=en"),
            "{method}"
        );
    }
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
        agent.fetch_text(&url).is_err(),
        "non-2xx must be an error for RateLimitedAgent"
    );
}

#[test]
fn test_flare_client_classifies_http_error() {
    // FlareClient's lenient agent must receive non-2xx bodies so
    // challenge classification can inspect them.
    let url = serve_once(NOT_FOUND_RESPONSE);
    let client = FlareClient::new(FlareClientConfig::default());
    match client.try_direct_get(&url, &client.operation()).unwrap() {
        DirectResult::HttpError(status, _) => assert_eq!(status, 404),
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
    let client = FlareClient::new(FlareClientConfig::default());
    let err = client.fetch_text(&url).unwrap_err();
    assert!(
        err.to_string().contains("404"),
        "error should carry the HTTP status: {err:#}"
    );
}

#[test]
fn test_should_escalate_status() {
    assert!(should_escalate_status(403));
    assert!(!should_escalate_status(429));
    assert!(should_escalate_status(503));
    assert!(!should_escalate_status(404));
    assert!(!should_escalate_status(500));
    assert!(!should_escalate_status(200));
}

// --- RateLimitedAgent --------------------------------------------------

// --- RateLimiter -------------------------------------------------------

#[test]
fn test_rate_limiter_valid_rps() {
    let limiter = RateLimiter::new(10.0);
    assert!(limiter.is_ok());
}

#[test]
fn test_rate_limiter_zero_rps() {
    assert!(RateLimiter::new(0.0).is_err());
}

#[test]
fn test_rate_limiter_negative_rps() {
    assert!(RateLimiter::new(-1.0).is_err());
}

#[test]
fn test_rate_limiter_nan() {
    assert!(RateLimiter::new(f64::NAN).is_err());
}

#[test]
fn test_rate_limiter_infinity() {
    assert!(RateLimiter::new(f64::INFINITY).is_err());
}

#[test]
fn test_rate_limiter_acquire_does_not_block_first_call() {
    let limiter = RateLimiter::new(1000.0).unwrap(); // high RPS
    let start = std::time::Instant::now();
    limiter.acquire(&Operation::new(None)).unwrap();
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
    limiter.acquire(&Operation::new(None)).unwrap(); // first: instant
    let start = std::time::Instant::now();
    limiter.acquire(&Operation::new(None)).unwrap(); // second: should wait ~100ms
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() >= 80, // some tolerance
        "Second acquire should have waited ~100ms, took {:?}",
        elapsed
    );
}

// --- build_image_get ---------------------------------------------------

// --- DirectResult via try_direct_get -----------------------------------

#[test]
fn test_direct_result_enum_variants() {
    // Ensure the enum is constructable (compile-time check mostly)
    let success = DirectResult::Success(FetchedDocument {
        final_url: "https://example.test/".into(),
        body: "hello".into(),
    });
    let challenged = DirectResult::Challenged(403);
    let http_error = DirectResult::HttpError(404, Duration::ZERO);

    match success {
        DirectResult::Success(s) => assert_eq!(s.body, "hello"),
        _ => panic!("Expected Success"),
    }

    match challenged {
        DirectResult::Challenged(code) => assert_eq!(code, 403),
        _ => panic!("Expected Challenged"),
    }

    match http_error {
        DirectResult::HttpError(code, _) => assert_eq!(code, 404),
        _ => panic!("Expected HttpError"),
    }
}

// --- build_rate_limited_flaresolverr_client without env -----------------

#[test]
fn test_build_rate_limited_flaresolverr_client_no_env() {
    isolated_env_case("builder-absent", None, None, || {
        let client = build_rate_limited_flaresolverr_client("https://example.com", Some(3.0));
        let guard = client.inner.lock().unwrap();
        assert!(guard.flaresolverr_url.is_none());
        assert!(guard.limiter.is_some());
    });
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
fn test_shared_session_navigation_keeps_each_document() {
    use std::{
        collections::HashMap,
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        sync::Barrier,
    };

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let config = FlareClientConfig {
        origin_url: "https://example.test/solve".into(),
        solver_url: Some(endpoint.clone()),
        session: FlareSession::Managed("shared-browser".into()),
        ..Default::default()
    };
    let first = FlareClient::new(config.clone());
    let clients = [
        first.clone(),
        first,
        FlareClient::new(config.clone()),
        FlareClient::new(FlareClientConfig {
            session: FlareSession::External("shared-browser".into()),
            ..config
        }),
    ];
    for client in &clients {
        client.disable_direct_path();
    }
    let worker = std::thread::spawn(move || {
        let pages = Arc::new(Mutex::new(HashMap::new()));
        let mut handlers = Vec::new();
        // Two independent managed clients discover the existing session;
        // four requests then navigate its single mutable browser document.
        let deadline = Instant::now() + Duration::from_secs(5);
        for _ in 0..6 {
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "navigation stub timed out");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            };
            let pages = pages.clone();
            handlers.push(std::thread::spawn(move || {
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut length = 0;
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                }
                assert!(length < 16384);
                let mut payload = vec![0; length];
                reader.read_exact(&mut payload).unwrap();
                let payload: JsonValue = serde_json::from_slice(&payload).unwrap();
                let response = if payload["cmd"] == "sessions.list" {
                    json!({"status":"ok", "sessions":["shared-browser"]})
                } else {
                    let session = payload["session"].as_str().unwrap().to_string();
                    assert_eq!(session, "shared-browser");
                    pages
                        .lock()
                        .unwrap()
                        .insert(session.clone(), payload["url"].clone());
                    std::thread::sleep(Duration::from_millis(100));
                    let page = pages.lock().unwrap()[&session].clone();
                    json!({"status":"ok", "solution":{"url":page, "status":200,
                        "cookies":[], "userAgent":"SolvedUA", "headers":{}, "response":page}})
                }
                .to_string();
                write!(
                    reader.get_mut(),
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                )
                .unwrap();
            }));
        }
        for handler in handlers {
            handler.join().unwrap();
        }
    });
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = clients
        .into_iter()
        .enumerate()
        .map(|(index, client)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let url = format!("https://example.test/{index}");
                barrier.wait();
                if index == 3 {
                    let endpoint = client.lock_inner().flaresolverr_url.clone().unwrap();
                    let solved = solve_with_flaresolverr(
                        &endpoint,
                        &url,
                        Some("shared-browser"),
                        &client.operation(),
                    )
                    .unwrap();
                    assert_eq!(solved.url, url);
                } else {
                    let document = match index {
                        0 => client.fetch_document(&url),
                        1 => client.post_form_document(&url, &[("q", "value")]),
                        _ => client.post_empty_document(&url, &[]),
                    }
                    .unwrap();
                    assert_eq!(document.final_url, url);
                    assert_eq!(document.body, url);
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    worker.join().unwrap();

    let operation = Operation::new(None);
    let lock = super::session::browser_lock(&endpoint, "shared-browser", &operation).unwrap();
    let _held = lock.lock().unwrap();
    let error = proxy_fetch_document(
        &endpoint,
        Some("shared-browser"),
        "https://example.test/",
        &Operation::with_timeout(Duration::from_millis(20)),
    )
    .unwrap_err();
    assert!(error.to_string().contains("deadline"));
    // A different session or endpoint must not queue behind the held browser.
    for (endpoint, session) in [
        (&*endpoint, "other"),
        ("http://other.test/", "shared-browser"),
    ] {
        let other = super::session::browser_lock(endpoint, session, &operation).unwrap();
        assert!(other.try_lock().is_ok());
    }
}

#[test]
fn test_flare_client_concurrent_re_solve_shares_outcome() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        sync::Barrier,
    };
    for succeeds in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let client = FlareClient::new(FlareClientConfig {
            origin_url: "https://example.test".into(),
            solver_url: Some(endpoint),
            session: FlareSession::External("test-session".into()),
            ..Default::default()
        });
        let body = if succeeds {
            json!({"status":"ok","solution":{"url":"https://example.test/","status":200,"cookies":[],"userAgent":"SolvedUA","headers":{},"response":"content"}})
        } else {
            json!({"status":"error","message":"The session doesn't exist."})
        }.to_string();
        let worker = std::thread::spawn(move || {
            // Exactly two RPCs: one shared attempt, then one later fresh attempt.
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut length = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                }
                let mut payload = vec![0; length];
                reader.read_exact(&mut payload).unwrap();
                assert_eq!(
                    serde_json::from_slice::<JsonValue>(&payload).unwrap()["session"],
                    "test-session"
                );
                write!(
                    reader.get_mut(),
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let attempt = client.pending_solve();
        let barrier = Arc::new(Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let client = client.clone();
                let attempt = attempt.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    client.re_solve(&attempt, &client.operation())
                })
            })
            .collect();
        for handle in handles {
            let result = handle.join().unwrap();
            if succeeds {
                assert!(result.unwrap());
            } else {
                assert!(is_missing_flaresolverr_session(&result.unwrap_err()));
            }
        }
        let next_attempt = client.pending_solve();
        assert!(!Arc::ptr_eq(&attempt, &next_attempt));
        assert_eq!(
            client.re_solve(&next_attempt, &client.operation()).is_ok(),
            succeeds
        );
        worker.join().unwrap();
    }
}

// =======================================================================
// Integration tests — require running FlareSolverr + network access
// Run with: cargo test -- --ignored
// =======================================================================

#[test]
#[ignore = "live source check"]
fn test_nowsecure() {
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
    let fs_url = flaresolverr_url();

    let client = FlareClient::new(FlareClientConfig {
        origin_url: "https://nowsecure.com".into(),
        solver_url: Some(fs_url),
        ..Default::default()
    });

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
    let client = FlareClient::new(FlareClientConfig::default());
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
    let fs_url = flaresolverr_url();
    let solved = solve_with_flaresolverr(
        &fs_url,
        "https://nowsecure.com",
        None,
        &Operation::new(None),
    )
    .unwrap();

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
    let client = FlareClient::new(FlareClientConfig::default());
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
    let client = FlareClient::new(FlareClientConfig::default());
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
    let fs_url = flaresolverr_url();

    let client = FlareClient::new(FlareClientConfig {
        origin_url: "https://nowsecure.com".into(),
        solver_url: Some(fs_url),
        ..Default::default()
    });
    let guard = client.inner.lock().unwrap();

    assert!(guard.session.current().is_none());
}

/// Integration: multiple sequential fetches reuse the same agent (direct path)
#[test]
#[ignore = "live source check"]
fn test_flare_client_multiple_fetches_reuse_agent() {
    let fs_url = flaresolverr_url();

    let client = FlareClient::new(FlareClientConfig {
        origin_url: "https://nowsecure.com".into(),
        solver_url: Some(fs_url),
        ..Default::default()
    });

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

#[test]
fn test_operation_deadline_bounds_pacing_solver_wait_and_response_body() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
    };
    let short_budget = || Operation::with_timeout(Duration::from_millis(30));
    let limiter = RateLimiter::new(0.1).unwrap();
    limiter.acquire(&Operation::new(None)).unwrap();
    assert!(
        limiter
            .acquire(&short_budget())
            .unwrap_err()
            .to_string()
            .contains("deadline")
    );

    let attempt = Arc::new(SolveAttempt::default());
    let leader_attempt = attempt.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let leader = std::thread::spawn(move || {
        leader_attempt
            .run(&Operation::new(None), || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(true)
            })
            .unwrap()
    });
    started_rx.recv().unwrap();
    let waiter_result = attempt.run(&short_budget(), || panic!("waiter must not solve"));
    release_tx.send(()).unwrap();
    assert!(leader.join().unwrap());
    assert!(waiter_result.unwrap_err().to_string().contains("deadline"));
    assert!(
        attempt
            .run(&Operation::new(None), || panic!(
                "completed solve must be reused"
            ))
            .unwrap()
    );

    // A request timeout must continue through body reads, not end at headers.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0; 4096];
        stream.read(&mut buffer).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\n")
            .unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let _ = stream.write_all(b"content");
    });
    let client = FlareClient::new(FlareClientConfig::default());
    let result = client.try_direct_get(&url, &short_budget());
    worker.join().unwrap();
    assert!(matches!(result, Err(error) if format!("{error:#}").contains("timeout")));
}

#[test]
fn test_rate_limit_backoff_cannot_bypass_budget_or_escalate_to_solver() {
    let mut headers = ureq::http::HeaderMap::new();
    assert_eq!(
        crate::backoff::retry_after(&headers),
        Duration::from_secs(5)
    );
    for (value, expected) in [
        ("0", Duration::ZERO),
        ("3", Duration::from_secs(3)),
        ("invalid", Duration::from_secs(5)),
        ("-1", Duration::from_secs(5)),
        ("999999999999999999999999999999", Duration::MAX),
        ("Sun, 06 Nov 1994 08:49:37 GMT", Duration::ZERO),
    ] {
        headers.insert("retry-after", value.parse().unwrap());
        assert_eq!(crate::backoff::retry_after(&headers), expected);
    }
    let operation = Operation::new(None);
    operation.backoff(Duration::ZERO).unwrap();
    assert!(solve::error_is::<crate::backoff::RateLimited>(
        &operation.backoff(Duration::ZERO).unwrap_err()
    ));
    for response in [
        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 240\r\nContent-Length: 9\r\nConnection: close\r\n\r\nslow down",
        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 240\r\nContent-Length: 900\r\nConnection: close\r\n\r\ntruncated",
    ] {
        let url = serve_once(response);
        let client = FlareClient::new(FlareClientConfig {
            origin_url: url.clone(),
            solver_url: Some("http://127.0.0.1:1/unused-solver".into()),
            ..Default::default()
        });
        let error = client.fetch_text(&url).unwrap_err();
        assert!(solve::error_is::<crate::backoff::RateLimited>(&error));
        assert!(format!("{error:#}").contains("remaining network operation deadline"));
        assert!(matches!(
            client.lock_inner().route,
            RouteState::DirectEligible
        ));
    }
}

#[test]
fn test_image_200_challenge_solves_once_before_retrying_bytes() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
    };
    for remains_challenged in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let solution = json!({"status":"ok","solution":{
            "url":format!("{origin}/"),"status":200,"cookies":[],"userAgent":"SolvedUA",
            "headers":{},"response":"solved origin"
        }})
        .to_string();
        let worker = std::thread::spawn(move || {
            for step in 0..3 {
                let deadline = Instant::now() + Duration::from_secs(5);
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "missing request at step {step}");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with(if step == 1 {
                    "POST /solver "
                } else {
                    "GET /image "
                }));
                let mut length = 0;
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut payload = vec![0; length];
                reader.read_exact(&mut payload).unwrap();
                if step == 1 {
                    assert_eq!(
                        serde_json::from_slice::<JsonValue>(&payload).unwrap()["cmd"],
                        "request.get"
                    );
                }
                let (mime, body) = if step == 1 {
                    ("application/json", solution.as_str())
                } else if step == 0 || remains_challenged {
                    (
                        "text/html",
                        "<html>Cloudflare: Just a moment<img src='/logo.png'></html>",
                    )
                } else {
                    ("image/gif", "GIF89a")
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                reader.get_mut().write_all(response.as_bytes()).unwrap();
            }
        });
        let client = FlareClient::new(FlareClientConfig {
            origin_url: origin.clone(),
            solver_url: Some(format!("{origin}/solver")),
            ..Default::default()
        });
        // Even explicitly allowed wrappers must never follow a challenge logo.
        let result = client.fetch_bytes_with_wrapper_policy(
            &format!("{origin}/image"),
            crate::ImageWrapperPolicy::FirstImage,
        );
        worker.join().unwrap();
        if remains_challenged {
            assert!(result.unwrap_err().is::<crate::image::ImageChallenge>());
        } else {
            assert_eq!(result.unwrap().as_ref(), b"GIF89a");
        }
    }
}
