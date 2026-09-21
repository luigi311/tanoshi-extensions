use crate::{
    ID, URL,
    api::{CdnConfigResponse, GalleryApiResponse},
};
use anyhow::{Context, Result, anyhow};
use chrono::DateTime;
use extension_utils::{element_text as trimmed_element_text, optional_text, unique_text};
use networking::FetchedDocument;
use scraper::{Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};
use url::Url;

pub(super) fn parse_cdn_server(config: CdnConfigResponse) -> Result<Url> {
    // The first server is source policy: an invalid entry must not select a fallback.
    let server = config
        .image_servers
        .first()
        .context("NHentai CDN API returned no image servers")?
        .trim();
    anyhow::ensure!(
        !server.contains('\\') && !server.chars().any(char::is_control),
        "NHentai CDN first image server contains a backslash or control character"
    );
    let server =
        Url::parse(server).context("NHentai CDN first image server is not a valid absolute URL")?;
    anyhow::ensure!(
        matches!(server.scheme(), "http" | "https") && server.host_str().is_some(),
        "NHentai CDN first image server must use HTTP(S) with a host"
    );
    anyhow::ensure!(
        server.username().is_empty()
            && server.password().is_none()
            && server.query().is_none()
            && server.fragment().is_none(),
        "NHentai CDN first image server must be a base URL without credentials, query or fragment"
    );
    Ok(server)
}

pub(super) fn gallery_id(path: &str) -> Result<&str> {
    path.split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_matches('/')
        .strip_prefix("g/")
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .with_context(|| format!("invalid NHentai gallery path: {path}"))
}

/// Validated source metadata shared by manga detail and synthetic chapter mapping.
/// Stored identities are supplied by the caller rather than cached here.
#[derive(Clone)]
pub(super) struct Gallery {
    title: String,
    author: Vec<String>,
    genre: Vec<String>,
    description: Option<String>,
    cover_url: String,
    scanlator: Option<String>,
    uploaded: i64,
}

impl Gallery {
    pub(super) fn into_manga(self, path: String) -> MangaInfo {
        MangaInfo {
            source_id: ID,
            title: self.title,
            author: self.author,
            genre: self.genre,
            description: self.description,
            cover_url: self.cover_url,
            status: None,
            path,
        }
    }

    pub(super) fn into_chapter(self, path: String) -> ChapterInfo {
        ChapterInfo {
            source_id: ID,
            title: "Chapter 1".into(),
            path,
            number: 1.0,
            scanlator: self.scanlator,
            uploaded: self.uploaded,
        }
    }
}

pub(super) fn parse_uploaded_timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.timestamp())
}

pub(super) fn build_gallery_page_urls(
    gallery_id: &str,
    gallery: &GalleryApiResponse,
    image_server: &Url,
) -> Result<Vec<String>> {
    if gallery.pages.is_empty() {
        return Err(anyhow!(
            "gallery {gallery_id} ({}) API response contains no pages",
            gallery.media_id
        ));
    }

    let image_server = image_server.as_str().trim_end_matches('/');

    let mut pages = gallery.pages.iter().collect::<Vec<_>>();
    pages.sort_by_key(|page| page.number);
    for (index, page) in pages.iter().enumerate() {
        let expected = index + 1;
        anyhow::ensure!(
            u64::from(page.number) == expected as u64,
            "gallery {gallery_id} ({}) API response has invalid page sequence: expected page {expected}, found {}; pages must be consecutive starting at 1",
            gallery.media_id,
            page.number
        );
    }

    pages
        .iter()
        .map(|page| {
            if page.path.trim().is_empty() {
                return Err(anyhow!(
                    "gallery {gallery_id} API response contains an empty page path"
                ));
            }
            Ok(format!(
                "{}/{}",
                image_server,
                page.path.trim_start_matches('/')
            ))
        })
        .collect()
}

pub(super) fn parse_manga_list(res: &FetchedDocument, url: &str) -> Result<Vec<MangaInfo>> {
    let document = Html::parse_document(&res.body);
    let gallery_selector =
        Selector::parse(".gallery").map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
    let image_selector =
        Selector::parse("a > img").map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
    let path_selector =
        Selector::parse("a").map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
    let title_selector =
        Selector::parse("a > .caption").map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
    let empty_selector = Selector::parse("#content .no-results > h2").unwrap();

    let mut manga_list = vec![];
    let matched = document.select(&gallery_selector).count();
    for (index, gallery) in document.select(&gallery_selector).enumerate() {
        let card = index + 1;
        // Missing artwork does not invalidate a gallery.
        let cover_url = gallery
            .select(&image_selector)
            .flat_map(|thumbnail| thumbnail.value().attr("src"))
            .next()
            .map(str::trim)
            .filter(|src| !src.is_empty())
            .map(|src| extension_utils::resolve_cover_url(&res.final_url, Some(src)))
            .unwrap_or_default();

        let path = gallery
            .select(&path_selector)
            .flat_map(|link| link.value().attr("href"))
            .next()
            .context("missing gallery link")
            .and_then(|href| extension_utils::source_link_path(URL, &res.final_url, href))
            .and_then(|path| {
                gallery_id(&path)?;
                Ok(path)
            });
        let path = match path {
            Ok(path) => path,
            Err(error) => {
                log::warn!("Skipping NHentai manga card {card} from {url}: {error:#}");
                continue;
            }
        };

        let Some(title) = gallery
            .select(&title_selector)
            .next()
            .and_then(trimmed_element_text)
        else {
            log::warn!("Skipping NHentai manga card {card} from {url}: missing title");
            continue;
        };

        manga_list.push(MangaInfo {
            source_id: ID,
            status: None,
            title,
            author: vec![],
            genre: vec![],
            description: None,
            path,
            cover_url,
        });
    }
    let rejected = matched - manga_list.len();
    if rejected > 0 {
        log::warn!("NHentai listing from {url}: rejected {rejected} of {matched} cards");
    }
    if manga_list.is_empty() {
        let recognized_empty = matched == 0
            && document
                .select(&empty_selector)
                .any(|el| trimmed_element_text(el).as_deref() == Some("No results found"));
        anyhow::ensure!(
            recognized_empty,
            "NHentai listing from {url}: no valid manga ({matched} matched, {rejected} rejected); unrecognized or malformed response"
        );
    }

    Ok(manga_list)
}

pub(super) fn parse_gallery(res: &FetchedDocument, path: &str) -> Result<Gallery> {
    let expected_id = gallery_id(path)?;
    let document = Html::parse_document(&res.body);
    let gallery_id_selector = Selector::parse("#info h3#gallery_id").unwrap();
    let title_selector = Selector::parse("#info h1.title > .pretty").unwrap();
    let displayed_id = document
        .select(&gallery_id_selector)
        .next()
        .with_context(|| format!("NHentai gallery {URL}{path}: missing gallery id"))?
        .text()
        .collect::<String>();
    anyhow::ensure!(
        displayed_id.trim().trim_start_matches('#').trim() == expected_id,
        "NHentai gallery {URL}{path}: response gallery id does not match requested id"
    );
    let title = document
        .select(&title_selector)
        .next()
        .and_then(trimmed_element_text)
        .with_context(|| format!("NHentai gallery {URL}{path}: missing title"))?;
    let thumbnail_selector = Selector::parse("#cover > a > img").unwrap();
    let author_selector = Selector::parse("a[href^=\"/artist/\"] > .name").unwrap();
    let genre_selector = Selector::parse("a[href^=\"/tag/\"] > .name").unwrap();
    let pages_selector = Selector::parse("a[href^=\"/search/?q=pages\"] > .name").unwrap();

    let mut description = displayed_id;
    for (label, selector) in [
        ("Parodies", "a[href^=\"/parody/\"] > .name"),
        ("Characters", "a[href^=\"/character/\"] > .name"),
        ("Languages", "a[href^=\"/language/\"] > .name"),
        ("Categories", "a[href^=\"/category/\"] > .name"),
    ] {
        let selector = Selector::parse(selector).unwrap();
        let values = document
            .select(&selector)
            .filter_map(trimmed_element_text)
            .collect::<Vec<_>>()
            .join(",");
        if !values.is_empty() {
            description.push_str(&format!("\n{label}: {values}"));
        }
    }
    if let Some(pages) = document.select(&pages_selector).next() {
        description.push_str(&format!("\nPages: {}", pages.text().collect::<String>()));
    }

    let cover_url = document
        .select(&thumbnail_selector)
        .flat_map(|el| el.value().attr("src"))
        .next()
        .map(str::trim)
        .filter(|src| !src.is_empty())
        .map(|src| extension_utils::resolve_cover_url(&res.final_url, Some(src)))
        .unwrap_or_default();

    let author: Vec<String> = document
        .select(&author_selector)
        .filter_map(trimmed_element_text)
        .collect::<Vec<String>>();

    let genre: Vec<String> = document
        .select(&genre_selector)
        .filter_map(trimmed_element_text)
        .collect::<Vec<String>>();

    let scanlator_selector = Selector::parse("a[href^=\"/group/\"] > .name")
        .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
    let uploaded_selector =
        Selector::parse(".tags > time").map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
    let scanlator = document
        .select(&scanlator_selector)
        .filter_map(trimmed_element_text)
        .next();
    let uploaded = if let Some(uploaded) = document.select(&uploaded_selector).next() {
        uploaded
            .value()
            .attr("datetime")
            .and_then(parse_uploaded_timestamp)
    } else {
        None
    };

    Ok(Gallery {
        title,
        author: unique_text(author),
        genre,
        description: optional_text(description),
        cover_url,
        scanlator,
        uploaded: uploaded.unwrap_or(0),
    })
}
