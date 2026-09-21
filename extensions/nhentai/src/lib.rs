mod api;
mod cache;
mod parse;
mod query;

use anyhow::Result;
use cache::{CdnConfigCache, GalleryCache};
use networking::{
    FlareClient, RateLimitedAgent, build_rate_limited_flaresolverr_client_for_extension,
    build_rate_limited_ureq_agent,
};
use query::{FILTER_LIST, PREFERENCES};
use std::sync::Mutex;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};
use urlencoding::encode;

const ID: i64 = 6;
const NAME: &str = "nhentai";
const URL: &str = "https://nhentai.net";
const ICON_URL: &str = "https://nhentai.net/static/img/logo.14bbfa78d3d0.svg";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REQUESTS_PER_SECOND: f64 = 10.0;

extension_utils::export_extension!(register, NHentai, NAME);

pub struct NHentai {
    preferences: Vec<Input>,
    client: FlareClient,
    image_client: RateLimitedAgent,
    gallery_cache: Mutex<GalleryCache>,
    cdn_cache: Mutex<CdnConfigCache>,
}

impl Default for NHentai {
    fn default() -> Self {
        Self {
            preferences: PREFERENCES.clone(),
            client: build_rate_limited_flaresolverr_client_for_extension(
                URL,
                Some(REQUESTS_PER_SECOND),
                "nhentai",
            ),
            image_client: build_rate_limited_ureq_agent(None, Some(REQUESTS_PER_SECOND)),
            gallery_cache: Mutex::new(GalleryCache::default()),
            cdn_cache: Mutex::new(CdnConfigCache::default()),
        }
    }
}

impl Extension for NHentai {
    extension_utils::impl_preferences!(preferences);

    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: VERSION,
            icon: ICON_URL,
            languages: Lang::Multi(vec!["en".to_string(), "ja".to_string(), "zh".to_string()]),
            nsfw: true,
        }
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_popular_manga page={page}");
        let request = self.query_parts(None, None)?;
        let q = encode(&request.text);
        self.get_manga_list(&format!("{URL}/search/?q={q}&sort=popular&page={page}"))
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_latest_manga page={page}");
        let request = self.query_parts(None, None)?;
        let q = encode(&request.text);
        self.get_manga_list(&format!("{URL}/search/?q={q}&page={page}"))
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: search_manga page={page} query={query:?}");
        let url = self.search_url(page, query, filters)?;
        self.get_manga_list(&url)
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        log::debug!("{NAME}: get_manga_detail path={path}");
        Ok(self.fetch_gallery(&path)?.into_manga(path))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        log::debug!("{NAME}: get_chapters path={path}");
        Ok(vec![self.fetch_gallery(&path)?.into_chapter(path)])
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        self.fetch_pages(path)
    }

    fn filter_list(&self) -> Vec<Input> {
        FILTER_LIST.clone()
    }

    extension_utils::impl_direct_image_fetch!(image_client, NAME, URL);
}

#[cfg(test)]
mod test;
