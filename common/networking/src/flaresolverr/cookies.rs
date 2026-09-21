use super::protocol::FlareSolverrCookie;
use crate::client::Agent;
use anyhow::{Context, Result, anyhow};
use cookie::time::OffsetDateTime as CookieOffsetDateTime;
use log::{debug, warn};
use ureq::{Cookie, http::Uri};
use url::Url;

pub(super) fn insert_flaresolverr_cookies_into_agent(
    agent: &Agent,
    solved_url: &str,
    cookies: Vec<FlareSolverrCookie>,
) -> Result<()> {
    let origin =
        Url::parse(solved_url).context("FlareSolverr returned an invalid cookie origin")?;
    if !matches!(origin.scheme(), "http" | "https") || origin.host_str().is_none() {
        return Err(anyhow!("FlareSolverr cookie origin must be an HTTP(S) URL"));
    }
    let uri = Uri::try_from(origin.as_str()).context("invalid FlareSolverr cookie origin URI")?;
    let now = CookieOffsetDateTime::now_utc();
    let mut jar = agent.cookie_jar_lock();
    for (index, c) in cookies.into_iter().enumerate() {
        let mut cookie = cookie::Cookie::build((c.name, c.value))
            .secure(c.secure)
            .http_only(c.httpOnly);
        if !c.path.is_empty() {
            cookie = cookie.path(c.path);
        }
        // Chrome's exported cookie domains use a leading dot for domain
        // cookies. Undotted domains (or an omitted domain) are host-only.
        if !c.domain.is_empty() {
            let is_domain_cookie = c.domain.starts_with('.');
            let raw_domain = c.domain.strip_prefix('.').unwrap_or(&c.domain);
            let Ok(domain) = url::Host::parse(raw_domain) else {
                warn!("FlareSolverr: skipped cookie {index}: invalid domain");
                continue;
            };
            if is_domain_cookie {
                let url::Host::Domain(domain) = domain else {
                    warn!(
                        "FlareSolverr: skipped cookie {index}: domain cookie requires a hostname"
                    );
                    continue;
                };
                if domain.ends_with('.') || psl::suffix_str(&domain) == Some(domain.as_str()) {
                    warn!("FlareSolverr: skipped cookie {index}: public suffix or invalid domain");
                    continue;
                }
                cookie = cookie.domain(domain);
            } else if origin.host().map(|host| host.to_owned()) != Some(domain) {
                warn!("FlareSolverr: skipped cookie {index}: host-only origin mismatch");
                continue;
            }
        }
        if let Some(expiry) = c.expiry {
            let Some(expires) = i64::try_from(expiry)
                .ok()
                .and_then(|timestamp| CookieOffsetDateTime::from_unix_timestamp(timestamp).ok())
            else {
                warn!("FlareSolverr: skipped cookie {index}: invalid expiry");
                continue;
            };
            if expires <= now {
                debug!("FlareSolverr: skipped cookie {index}: expired");
                continue;
            }
            cookie = cookie.expires(expires);
        }
        cookie = match c.sameSite.as_str() {
            "Strict" => cookie.same_site(cookie::SameSite::Strict),
            "Lax" => cookie.same_site(cookie::SameSite::Lax),
            "None" => cookie.same_site(cookie::SameSite::None),
            _ => cookie,
        };
        // Bind every cookie to the actual final URL. The jar validates domain
        // matching and supplies defaults for host-only cookies and empty paths.
        // Log rejection categories only; parser errors can contain cookie data.
        match Cookie::parse(cookie.build().to_string(), &uri) {
            Ok(cookie) => {
                if jar.insert(cookie, &uri).is_err() {
                    warn!("FlareSolverr: skipped cookie {index}: rejected by cookie jar");
                }
            }
            Err(_) => {
                warn!("FlareSolverr: skipped cookie {index}: invalid cookie or domain mismatch")
            }
        }
    }
    Ok(())
}
