//! HTTP clients shared by the source extensions.
mod backoff;
mod client;
mod flaresolverr;
mod image;
mod operation;
mod ratelimit;

pub use image::ImageWrapperPolicy;

pub use client::{FetchedDocument, RateLimitedAgent, build_rate_limited_ureq_agent};
pub use flaresolverr::{
    FlareClient, FlareClientConfig, FlareSession, build_rate_limited_flaresolverr_client,
    build_rate_limited_flaresolverr_client_for_extension, parse_browser_json,
};

use std::sync::Once;

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
