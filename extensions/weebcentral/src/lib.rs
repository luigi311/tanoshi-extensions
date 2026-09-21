mod parse;
mod query;

use anyhow::{Context, Result};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use query::{ListingQuery, Sort};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

extension_utils::export_extension!(register, Weebcentral, NAME);

const ID: i64 = 28;
const NAME: &str = "WeebCentral";
const URL: &str = "https://weebcentral.com";
const ICON_URL: &str = "https://weebcentral.com/static/images/144.png";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REQUESTS_PER_SECOND: f64 = 10.0;
// Get Pages seems to have its own rate limit
const PAGES_REQUESTS_PER_SECOND: f64 = 1.0;

pub struct Weebcentral {
    client: RateLimitedAgent,
    client_pages: RateLimitedAgent,
}

impl Default for Weebcentral {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(None, Some(REQUESTS_PER_SECOND)),
            client_pages: build_rate_limited_ureq_agent(None, Some(PAGES_REQUESTS_PER_SECOND)),
        }
    }
}

fn get_manga_list(query: ListingQuery<'_>, client: &RateLimitedAgent) -> Result<Vec<MangaInfo>> {
    let url = query.url()?;
    let response = client
        .fetch_document(&url)
        .with_context(|| format!("WeebCentral listing request failed: {url}"))?;
    parse::parse_manga_list(&response, &url)
}

impl Extension for Weebcentral {
    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: VERSION,
            icon: ICON_URL,
            languages: Lang::Single("en".to_string()),
            nsfw: false,
        }
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_popular_manga page={page}");
        get_manga_list(
            ListingQuery {
                page,
                sort: Sort::Popularity,
                text: Some(""),
            },
            &self.client,
        )
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_latest_manga page={page}");
        get_manga_list(
            ListingQuery {
                page,
                sort: Sort::LatestUpdates,
                text: None,
            },
            &self.client,
        )
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: search_manga page={page} query={query:?}");
        //TODO: Add filters
        get_manga_list(
            ListingQuery {
                page,
                sort: Sort::LatestUpdates,
                text: Some(query.as_deref().unwrap_or_default()),
            },
            &self.client,
        )
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        log::debug!("{NAME}: get_manga_detail path={path}");
        let request_url = extension_utils::source_request_url(URL, &path)?;
        parse::source_id(&path, "series", URL).context("invalid WeebCentral series path")?;
        let response = self
            .client
            .fetch_document(request_url.as_str())
            .with_context(|| format!("WeebCentral detail request failed: {URL}{path}"))?;

        parse::parse_detail(&response, path)
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        log::debug!("{NAME}: get_chapters path={path}");
        let mut request_url = extension_utils::source_request_url(URL, &path)?;
        request_url.set_path(&format!(
            "{}/full-chapter-list",
            request_url.path().trim_end_matches('/')
        ));
        let response = self.client.fetch_document(request_url.as_str())?;

        parse::parse_chapters(&response, &path)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        log::debug!("{NAME}: get_pages path={path}");
        let mut request_url = extension_utils::source_request_url(URL, &path)?;
        request_url.set_path(&format!(
            "{}/images",
            request_url.path().trim_end_matches('/')
        ));
        request_url.query_pairs_mut().extend_pairs([
            ("is_prev", "False"),
            ("current_page", "1"),
            ("reading_style", "single_page"),
        ]);
        let response = self.client_pages.fetch_document(request_url.as_str())?;

        parse::parse_pages(&response, &path)
    }

    extension_utils::impl_direct_image_fetch!(client, NAME, URL);
}

#[cfg(test)]
mod test;
