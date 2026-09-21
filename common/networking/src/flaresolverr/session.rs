use super::{
    FlareClient, FlareSession,
    protocol::{flaresolverr_rpc, is_missing_flaresolverr_session},
};
use crate::{FetchedDocument, operation::Operation};
use anyhow::{Context, Result, anyhow, ensure};
use log::{debug, info, warn};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, Weak},
};

// Serializes discovery/create within this linked copy of networking. Other
// plugins and processes can still race; creation failures are re-listed below.
static FLARESOLVERR_SESSION_INIT_LOCK: Mutex<()> = Mutex::new(());

type BrowserLocks = HashMap<(String, String), Weak<Mutex<()>>>;
static BROWSER_LOCKS: LazyLock<Mutex<BrowserLocks>> = LazyLock::new(Mutex::default);

/// Coordinate browser navigation across clients in this linked library. Weak
/// entries keep completed sessions from accumulating in the registry.
pub(super) fn browser_lock(
    endpoint: &str,
    session: &str,
    operation: &Operation,
) -> Result<Arc<Mutex<()>>> {
    let mut endpoint = url::Url::parse(endpoint)?;
    endpoint.set_fragment(None);
    let key = (endpoint.to_string(), session.to_string());
    let mut locks = operation.lock(&BROWSER_LOCKS)?;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    Ok(lock)
}

#[derive(Clone)]
pub(super) enum SessionState {
    Stateless,
    External(Arc<Session>),
    Managed {
        name: String,
        current: Option<Arc<Session>>,
    },
}

// Arc identity distinguishes a recovered session from an older lease with the
// same server-side name. Late failures must not clear a newer recovered lease.
pub(super) struct Session {
    pub(super) id: String,
}

impl From<FlareSession> for SessionState {
    fn from(session: FlareSession) -> Self {
        match session {
            FlareSession::Stateless => Self::Stateless,
            FlareSession::External(id) => Self::External(Arc::new(Session { id })),
            FlareSession::Managed(name) => Self::Managed {
                name,
                current: None,
            },
        }
    }
}

impl SessionState {
    pub(super) fn current(&self) -> Option<Arc<Session>> {
        match self {
            Self::Stateless => None,
            Self::External(session) => Some(session.clone()),
            Self::Managed { current, .. } => current.clone(),
        }
    }

    pub(super) fn managed_name(&self) -> Option<&str> {
        match self {
            Self::Managed { name, .. } => Some(name),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct ManagedSessionUnavailable;

impl std::fmt::Display for ManagedSessionUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FlareSolverr managed session initialization/recovery failed")
    }
}

impl std::error::Error for ManagedSessionUnavailable {}

pub(super) fn is_managed_session_error(error: &anyhow::Error) -> bool {
    super::solve::error_is::<ManagedSessionUnavailable>(error)
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

fn list_flaresolverr_sessions(
    flaresolverr_url: &str,
    operation: &Operation,
) -> Result<Vec<String>> {
    let payload = json!({"cmd": "sessions.list"});
    let body: FlareSolverrSessionListResponse =
        serde_json::from_value(flaresolverr_rpc(flaresolverr_url, &payload, operation)?)?;
    Ok(body.sessions)
}

fn create_flaresolverr_session(
    flaresolverr_url: &str,
    session_name: &str,
    operation: &Operation,
) -> Result<String> {
    let payload = json!({
        "cmd": "sessions.create",
        "session": session_name,
    });
    let response = match flaresolverr_rpc(flaresolverr_url, &payload, operation) {
        Ok(response) => response,
        Err(error) => {
            // Another process may have created the name after sessions.list,
            // or creation may have succeeded before its response was lost.
            if list_flaresolverr_sessions(flaresolverr_url, operation)
                .is_ok_and(|sessions| sessions.iter().any(|id| id == session_name))
            {
                warn!(
                    "FlareClient: session creation failed but the requested name exists after re-listing; reusing {}: {:#}",
                    session_name, error
                );
                return Ok(session_name.to_string());
            }
            return Err(error);
        }
    };
    let body: FlareSolverrSessionCreateResponse = serde_json::from_value(response)?;
    let session = body.session.ok_or_else(|| {
        anyhow!("FlareSolverr sessions.create succeeded without returning a session ID")
    })?;
    ensure!(
        session == session_name,
        "FlareSolverr sessions.create did not return the requested session name"
    );
    Ok(session)
}

impl FlareClient {
    pub(super) fn session_for_request(
        &self,
        operation: &Operation,
    ) -> Result<Option<Arc<Session>>> {
        {
            let guard = self.lock_inner();
            if let Some(session) = guard.session.current() {
                return Ok(Some(session));
            }
            if guard.flaresolverr_url.is_none() || guard.session.managed_name().is_none() {
                return Ok(None);
            }
        }
        let _init_guard = operation.lock(&FLARESOLVERR_SESSION_INIT_LOCK)?;
        self.initialize_named_session(operation)
            .context(ManagedSessionUnavailable)
    }

    /// Caller holds the initialization lock; general client state is never
    /// locked during network I/O. Recheck state after waiting for initialization.
    fn initialize_named_session(&self, operation: &Operation) -> Result<Option<Arc<Session>>> {
        let (endpoint, name) = {
            let guard = self.lock_inner();
            if let Some(session) = guard.session.current() {
                return Ok(Some(session));
            }
            match (&guard.flaresolverr_url, guard.session.managed_name()) {
                (Some(endpoint), Some(name)) => (endpoint.clone(), name.to_string()),
                _ => return Ok(None),
            }
        };
        let id = if list_flaresolverr_sessions(&endpoint, operation)?
            .iter()
            .any(|id| id == &name)
        {
            debug!(
                "FlareClient: reusing existing FlareSolverr session {}",
                name
            );
            name
        } else {
            info!("FlareClient: creating FlareSolverr session {}", name);
            create_flaresolverr_session(&endpoint, &name, operation)?
        };
        let session = Arc::new(Session { id });
        if let SessionState::Managed { current, .. } = &mut self.lock_inner().session {
            *current = Some(session.clone());
        }
        Ok(Some(session))
    }

    pub(super) fn uses_named_session(&self) -> bool {
        self.lock_inner().session.managed_name().is_some()
    }

    pub(super) fn refresh_named_session(
        &self,
        failed: &Arc<Session>,
        operation: &Operation,
    ) -> Result<Arc<Session>> {
        let _init_guard = operation.lock(&FLARESOLVERR_SESSION_INIT_LOCK)?;
        {
            let mut guard = self.lock_inner();
            match &mut guard.session {
                SessionState::Managed { current, .. } => {
                    if current
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, failed))
                    {
                        *current = None;
                    }
                }
                _ => {
                    return Err(anyhow!(
                        "cannot recover an externally owned or stateless session"
                    ));
                }
            }
        }
        self.initialize_named_session(operation)
            .and_then(|session| {
                session.ok_or_else(|| anyhow!("managed session unavailable after recovery"))
            })
            .context(ManagedSessionUnavailable)
    }

    pub(super) fn proxy_with_session_retry<P>(
        &self,
        flaresolverr_url: &str,
        method: &str,
        url: &str,
        session: Option<&Arc<Session>>,
        proxy_request: &P,
        operation: &Operation,
    ) -> Result<FetchedDocument>
    where
        P: Fn(&str, Option<&str>, &str, &Operation) -> Result<FetchedDocument>,
    {
        match proxy_request(
            flaresolverr_url,
            session.map(|s| s.id.as_str()),
            url,
            operation,
        ) {
            Ok(text) => Ok(text),
            Err(error) if self.uses_named_session() && is_missing_flaresolverr_session(&error) => {
                let Some(failed) = session else {
                    return Err(error);
                };
                warn!(
                    "FlareClient: session disappeared while proxying {} {}, refreshing it: {:#}",
                    method, url, error
                );
                let refreshed = self.refresh_named_session(failed, operation)?;
                proxy_request(flaresolverr_url, Some(&refreshed.id), url, operation)
            }
            Err(error) => Err(error),
        }
    }
}
