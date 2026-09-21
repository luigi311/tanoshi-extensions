use anyhow::{Context, Result, anyhow};
use networking::{FetchedDocument, FlareClient, RateLimitedAgent};
use scraper::Selector;
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::parse::{
    parse_ajax_chapters, parse_detail, parse_html_chapters, parse_manga_list, parse_pages,
};

/// Detail requests support Manhwa18cc's direct client and the retained Madara
/// restoration candidates' solver client.
pub trait DetailClient {
    fn fetch_document(&self, url: &str) -> anyhow::Result<FetchedDocument>;
}

impl DetailClient for FlareClient {
    fn fetch_document(&self, url: &str) -> anyhow::Result<FetchedDocument> {
        self.fetch_document(url)
    }
}

impl DetailClient for RateLimitedAgent {
    fn fetch_document(&self, url: &str) -> anyhow::Result<FetchedDocument> {
        self.fetch_document(url)
    }
}

pub fn fetch_ajax_latest(
    url: &str,
    source_id: i64,
    page: i64,
    client: &FlareClient,
) -> Result<Vec<MangaInfo>> {
    let form: &[(&str, &str)] = &[
        ("action", "madara_load_more"),
        ("page", &(page - 1).to_string()),
        ("template", "madara-core/content/content-archive"),
        ("vars[orderby]", "meta_value_num"),
        ("vars[paged]", "1"),
        ("vars[posts_per_page]", "20"),
        ("vars[post_type]", "wp-manga"),
        ("vars[post_status]", "publish"),
        ("vars[meta_key]", "_latest_update"),
        ("vars[order]", "desc"),
        ("vars[sidebar]", "right"),
        ("vars[manga_archives_item_layout]", "big_thumbnail"),
        ("vars[meta_query][0][key]", "_wp_manga_chapter_type"),
        ("vars[meta_query][0][value]", "manga"),
    ];

    let response = client.post_form_document(&format!("{}/wp-admin/admin-ajax.php", url), form)?;

    let selector = Selector::parse("div.page-item-detail")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    parse_manga_list(url, source_id, &response, &selector)
}

pub fn fetch_ajax_popular(
    url: &str,
    source_id: i64,
    page: i64,
    client: &FlareClient,
) -> Result<Vec<MangaInfo>> {
    let form: &[(&str, &str)] = &[
        ("action", "madara_load_more"),
        ("page", &(page - 1).to_string()),
        ("template", "madara-core/content/content-archive"),
        ("vars[orderby]", "meta_value_num"),
        ("vars[paged]", "1"),
        ("vars[posts_per_page]", "20"),
        ("vars[post_type]", "wp-manga"),
        ("vars[post_status]", "publish"),
        ("vars[meta_key]", "_wp_manga_views"),
        ("vars[order]", "desc"),
        ("vars[sidebar]", "full"),
        ("vars[manga_archives_item_layout]", "big_thumbnail"),
        ("vars[meta_query][0][key]", "_wp_manga_chapter_type"),
        ("vars[meta_query][0][value]", "manga"),
    ];

    let response = client.post_form_document(&format!("{}/wp-admin/admin-ajax.php", url), form)?;

    let selector = Selector::parse("div.page-item-detail")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    parse_manga_list(url, source_id, &response, &selector)
}

pub fn search_html(
    url: &str,
    source_id: i64,
    page: i64,
    query: &str,
    client: &RateLimitedAgent,
) -> Result<Vec<MangaInfo>> {
    let query = urlencoding::encode(query);
    let request_url = format!("{}/search?q={}&page={}", url, query, page);
    let response = client
        .fetch_document(&request_url)
        .with_context(|| format!("Madara search request failed: {request_url}"))?;

    let selector =
        Selector::parse(".manga-item").map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    parse_manga_list(url, source_id, &response, &selector)
        .with_context(|| format!("Madara search response from {request_url}"))
}

pub fn search_ajax(
    url: &str,
    source_id: i64,
    page: i64,
    query: &str,
    client: &FlareClient,
) -> Result<Vec<MangaInfo>> {
    let form: &[(&str, &str)] = &[
        ("action", "madara_load_more"),
        ("vars[s]", query),
        ("template", "madara-core/content/content-search"),
        ("vars[paged]", "1"),
        ("vars[template]", "archive"),
        ("vars[post_type]", "wp-manga"),
        ("vars[post_status]", "publish"),
        ("vars[sidebar]", "right"),
        ("vars[manga_archives_item_layout]", "big_thumbnail"),
        ("vars[posts_per_page]", "20"),
        ("vars[meta_query][0][key]", "_wp_manga_chapter_type"),
        ("vars[meta_query][0][value]", "manga"),
        ("page", &(page - 1).to_string()),
    ];

    let response = client.post_form_document(&format!("{}/wp-admin/admin-ajax.php", url), form)?;

    let selector = Selector::parse("div.c-tabs-item__content")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    parse_manga_list(url, source_id, &response, &selector)
}

pub fn get_manga_detail<C: DetailClient>(
    url: &str,
    path: &str,
    source_id: i64,
    client: &C,
) -> Result<MangaInfo> {
    let request_url = extension_utils::source_request_url(url, path)?;
    let response = client.fetch_document(request_url.as_str())?;

    parse_detail(url, path, source_id, &response)
}

pub fn fetch_html_chapters(
    url: &str,
    path: &str,
    source_id: i64,
    client: &RateLimitedAgent,
) -> Result<Vec<ChapterInfo>> {
    let request_url = extension_utils::source_request_url(url, path)?;
    let response = client.fetch_document(request_url.as_str())?;

    parse_html_chapters(url, path, source_id, &response)
}

pub fn fetch_ajax_chapters(
    url: &str,
    path: &str,
    source_id: i64,
    chapter_name_selector: Option<&str>,
    client: &FlareClient,
) -> Result<Vec<ChapterInfo>> {
    let mut request_url = extension_utils::source_request_url(url, path)?;
    request_url.set_path(&format!(
        "{}/ajax/chapters",
        request_url.path().trim_end_matches('/')
    ));
    let response = client.post_empty_document(
        request_url.as_str(),
        &[
            ("Referer", url),
            ("Content-Length", "0"),
            ("X-Requested-With", "XMLHttpRequest"),
        ],
    )?;

    parse_ajax_chapters(url, source_id, chapter_name_selector, &response)
}

pub fn fetch_pages(url: &str, path: &str, client: &FlareClient) -> Result<Vec<String>> {
    let request_url = extension_utils::source_request_url(url, path)?;
    let response = client.post_empty_document(request_url.as_str(), &[("Referer", url)])?;

    parse_pages(url, path, &response)
}
