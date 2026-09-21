use anyhow::{Context, Result, anyhow};
use scraper::{Html, Selector};
use serde::de::DeserializeOwned;

/// Decode JSON returned directly or inside a browser-rendered `<pre>` element.
/// Use only for APIs fetched through the browser transport; ordinary JSON uses
/// `RateLimitedAgent::fetch_json`. `resource` supplies source-specific error context.
pub fn parse_browser_json<T: DeserializeOwned>(body: &str, resource: &str) -> Result<T> {
    let direct_error = match serde_json::from_str(body) {
        Ok(response) => return Ok(response),
        Err(error) => error,
    };

    let pre_selector = Selector::parse("pre")
        .map_err(|error| anyhow!("failed to parse FlareSolverr response selector: {error:?}"))?;
    let document = Html::parse_document(body);
    let wrapped_body = document
        .select(&pre_selector)
        .next()
        .map(|element| element.text().collect::<String>())
        .filter(|text| !text.trim().is_empty());
    let Some(wrapped_body) = wrapped_body else {
        return Err(direct_error)
            .with_context(|| format!("failed to parse {resource} API response"));
    };

    serde_json::from_str(&wrapped_body)
        .with_context(|| format!("failed to parse wrapped {resource} API response"))
}
