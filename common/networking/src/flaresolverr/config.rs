/// Session ownership requested by a solver client.
#[derive(Clone, Debug, Default)]
pub enum FlareSession {
    #[default]
    Stateless,
    /// Discover or lazily create this extension-owned session.
    Managed(String),
    /// Use a supplied session ID without client-side discovery or creation.
    /// FlareSolverr may recreate a missing session under this same ID.
    External(String),
}

/// Explicit construction settings. Construction does no network I/O.
#[derive(Clone, Debug, Default)]
pub struct FlareClientConfig {
    pub origin_url: String,
    pub requests_per_second: Option<f64>,
    pub solver_url: Option<String>,
    pub session: FlareSession,
}

impl FlareClientConfig {
    pub(super) fn validate_solver_url(&self) -> Result<(), &'static str> {
        let Some(endpoint) = &self.solver_url else {
            return Ok(());
        };
        let valid = !endpoint.chars().any(char::is_whitespace)
            && !endpoint.chars().any(char::is_control)
            && url::Url::parse(endpoint).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            });
        if valid {
            Ok(())
        } else {
            Err(
                "invalid FlareSolverr configuration: FLARESOLVERR_URL must be an absolute HTTP(S) URL with a host and no whitespace; unset it for direct-only operation",
            )
        }
    }

    /// Read the optional solver environment once. An explicit session takes
    /// precedence over the extension's managed name. No endpoint means direct
    /// requests only, and FLARESOLVERR_SESSION has no effect in that mode.
    pub fn from_env(
        origin_url: &str,
        requests_per_second: Option<f64>,
        session_name: Option<&str>,
    ) -> Self {
        // A present, non-UTF-8 value is invalid configuration, not an absent
        // endpoint. Preserve that distinction as an invalid empty URL.
        let solver_url = std::env::var_os("FLARESOLVERR_URL")
            .map(|value| value.into_string().unwrap_or_default());
        let explicit_session = solver_url.as_ref().and_then(|_| {
            std::env::var("FLARESOLVERR_SESSION")
                .ok()
                .filter(|session| !session.trim().is_empty())
        });
        let session = match explicit_session {
            Some(id) => FlareSession::External(id),
            None => session_name
                .filter(|name| !name.trim().is_empty())
                .map(|name| FlareSession::Managed(name.to_string()))
                .unwrap_or_default(),
        };
        Self {
            origin_url: origin_url.to_string(),
            requests_per_second,
            solver_url,
            session,
        }
    }
}
