use crate::{
    NAME, NHentai, URL,
    parse::{
        Gallery, build_gallery_page_urls, gallery_id, parse_cdn_server, parse_gallery,
        parse_manga_list,
    },
};
use anyhow::{Context, Result};
use networking::parse_browser_json;
use serde::Deserialize;
use std::time::Instant;
use tanoshi_lib::prelude::MangaInfo;
use url::Url;

#[derive(Debug, Deserialize)]
pub(super) struct GalleryApiResponse {
    pub(super) media_id: String,
    pub(super) pages: Vec<GalleryApiPage>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GalleryApiPage {
    pub(super) number: u32,
    pub(super) path: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CdnConfigResponse {
    pub(super) image_servers: Vec<String>,
}

impl NHentai {
    pub(super) fn fetch_gallery(&self, path: &str) -> Result<Gallery> {
        let url = extension_utils::source_request_url(URL, path)?.to_string();
        gallery_id(path)?;
        {
            let mut cache = self
                .gallery_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(gallery) = cache.get(&url, Instant::now()) {
                log::debug!("{NAME}: gallery cache hit url={url}");
                return Ok(gallery);
            }
        }

        let document = self
            .client
            .fetch_document(&url)
            .with_context(|| format!("NHentai gallery request failed: {url}"))?;
        // Both details and the synthetic chapter must refer to a real gallery.
        // Reject unexpected responses before they can populate the shared cache.
        let gallery = parse_gallery(&document, path)?;
        let mut cache = self
            .gallery_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.insert(url, gallery.clone(), Instant::now());
        Ok(gallery)
    }

    fn fetch_cdn_server(&self) -> Result<Url> {
        {
            let mut cache = self
                .cdn_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(image_server) = cache.get(Instant::now()) {
                log::debug!("{NAME}: CDN config cache hit");
                return Ok(image_server);
            }
        }

        let cdn_url = format!("{URL}/api/v2/cdn");
        let cdn_res = self
            .client
            .fetch_text(&cdn_url)
            .with_context(|| format!("NHentai CDN request failed: {cdn_url}"))?;
        let cdn: CdnConfigResponse = parse_browser_json(&cdn_res, "NHentai CDN")
            .with_context(|| format!("NHentai CDN response from {cdn_url}"))?;
        let image_server = parse_cdn_server(cdn)
            .with_context(|| format!("NHentai CDN response from {cdn_url}"))?;

        let mut cache = self
            .cdn_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.insert(image_server.clone(), Instant::now());
        Ok(image_server)
    }

    pub(super) fn get_manga_list(&self, url: &str) -> Result<Vec<MangaInfo>> {
        let response = self
            .client
            .fetch_document(url)
            .with_context(|| format!("NHentai listing request failed: {url}"))?;
        parse_manga_list(&response, url)
    }

    pub(super) fn fetch_pages(&self, path: String) -> Result<Vec<String>> {
        log::debug!("{NAME}: get_pages path={path}");
        let mut request_url = extension_utils::source_request_url(URL, &path)?;
        let gallery_id = gallery_id(&path)?;
        request_url.set_path(&format!("/api/v2/galleries/{gallery_id}"));
        let api_url = request_url.as_str();
        let gallery_res = self
            .client
            .fetch_text(&api_url)
            .with_context(|| format!("NHentai gallery API request failed: {api_url}"))?;
        let gallery: GalleryApiResponse = parse_browser_json(&gallery_res, "NHentai gallery")
            .with_context(|| format!("NHentai gallery API response from {api_url}"))?;

        let image_server = self.fetch_cdn_server()?;

        build_gallery_page_urls(gallery_id, &gallery, &image_server)
    }
}
