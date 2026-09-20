use anyhow::{Context, Result, anyhow};
use chrono::DateTime;
use lazy_static::lazy_static;
use networking::{
    FlareClient, RateLimitedAgent, build_rate_limited_flaresolverr_client_for_extension,
    build_rate_limited_ureq_agent,
};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, de::DeserializeOwned};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, InputType, Lang, MangaInfo, SourceInfo};
use urlencoding::encode;

const ID: i64 = 6;
const NAME: &str = "nhentai";
const URL: &str = "https://nhentai.net";
const ICON_URL: &str = "https://nhentai.net/static/img/logo.14bbfa78d3d0.svg";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REQUESTS_PER_SECOND: f64 = 10.0;
const GALLERY_CACHE_CAPACITY: usize = 4;
const GALLERY_CACHE_TTL: Duration = Duration::from_secs(15);
const CDN_CONFIG_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

extension_utils::export_extension!(register, NHentai, NAME);

lazy_static! {
    static ref TAG_FILTER: Input = Input::Text {
        name: "Tag".to_string(),
        state: None
    };
    static ref CHARACTERS_FILTER: Input = Input::Text {
        name: "Characters".to_string(),
        state: None
    };
    static ref ARTISTS_FILTER: Input = Input::Text {
        name: "Artists".to_string(),
        state: None
    };
    static ref GROUPS_FILTER: Input = Input::Text {
        name: "Groups".to_string(),
        state: None
    };
    static ref CATEGORIES_FILTER: Input = Input::Text {
        name: "Categories".to_string(),
        state: None
    };
    static ref PARODIES_FILTER: Input = Input::Text {
        name: "Parodies".to_string(),
        state: None
    };
    static ref SORT_FILTER: Input = Input::Select {
        name: "Sort".to_string(),
        values: vec![
            InputType::String("Popular".to_string()),
            InputType::String("Popular Week".to_string()),
            InputType::String("Popular Today".to_string()),
            InputType::String("Recent".to_string()),
        ],
        state: None
    };
    static ref FILTER_LIST: Vec<Input> = vec![
        TAG_FILTER.clone(),
        CHARACTERS_FILTER.clone(),
        CATEGORIES_FILTER.clone(),
        PARODIES_FILTER.clone(),
        ARTISTS_FILTER.clone(),
        GROUPS_FILTER.clone(),
        SORT_FILTER.clone()
    ];
    static ref LANGUAGE_SELECT: Input = Input::Select {
        name: "Language".to_string(),
        values: vec![
            InputType::String("Any".to_string()),
            InputType::String("English".to_string()),
            InputType::String("Japanese".to_string()),
            InputType::String("Chinese".to_string()),
        ],
        state: None
    };
    static ref BLACKLIST_TAG: Input = Input::Text {
        name: "Blacklist Tag".to_string(),
        state: None
    };
    static ref PREFERENCES: Vec<Input> = vec![LANGUAGE_SELECT.clone(), BLACKLIST_TAG.clone()];
}

struct CachedGalleryPage {
    url: String,
    body: String,
    fetched_at: Instant,
}

#[derive(Default)]
struct GalleryPageCache {
    entries: VecDeque<CachedGalleryPage>,
}

#[derive(Debug, Deserialize)]
struct GalleryApiResponse {
    media_id: String,
    pages: Vec<GalleryApiPage>,
}

#[derive(Debug, Deserialize)]
struct GalleryApiPage {
    number: u32,
    path: String,
}

#[derive(Debug, Deserialize)]
struct CdnConfigResponse {
    image_servers: Vec<String>,
}

struct CachedCdnConfig {
    image_servers: Vec<String>,
    fetched_at: Instant,
}

#[derive(Default)]
struct CdnConfigCache {
    entry: Option<CachedCdnConfig>,
}

fn parse_api_response<T: DeserializeOwned>(body: &str, resource: &str) -> Result<T> {
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
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            anyhow!("failed to parse NHentai {resource} API response: {direct_error}")
        })?;

    serde_json::from_str(&wrapped_body)
        .map_err(|error| anyhow!("failed to parse NHentai {resource} API response: {error}"))
}

impl CdnConfigCache {
    fn get(&mut self) -> Option<Vec<String>> {
        let entry = self.entry.as_ref()?;
        if entry.fetched_at.elapsed() > CDN_CONFIG_CACHE_TTL {
            self.entry = None;
            return None;
        }

        Some(entry.image_servers.clone())
    }

    fn insert(&mut self, image_servers: Vec<String>) {
        self.entry = Some(CachedCdnConfig {
            image_servers,
            fetched_at: Instant::now(),
        });
    }
}

impl GalleryPageCache {
    fn get(&mut self, url: &str) -> Option<String> {
        let index = self.entries.iter().position(|entry| entry.url == url)?;
        if self.entries[index].fetched_at.elapsed() > GALLERY_CACHE_TTL {
            self.entries.remove(index);
            return None;
        }

        let entry = self.entries.remove(index)?;
        let body = entry.body.clone();
        self.entries.push_front(entry);
        Some(body)
    }

    fn insert(&mut self, url: String, body: String) {
        self.entries.retain(|entry| entry.url != url);
        self.entries.push_front(CachedGalleryPage {
            url,
            body,
            fetched_at: Instant::now(),
        });
        self.entries.truncate(GALLERY_CACHE_CAPACITY);
    }
}

pub struct NHentai {
    preferences: Vec<Input>,
    client: FlareClient,
    image_client: RateLimitedAgent,
    gallery_cache: Mutex<GalleryPageCache>,
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
            gallery_cache: Mutex::new(GalleryPageCache::default()),
            cdn_cache: Mutex::new(CdnConfigCache::default()),
        }
    }
}

fn nh_field_key(ui_label: &str) -> Option<&'static str> {
    match ui_label {
        "Tag" => Some("tag"),
        "Characters" => Some("character"),
        "Artists" => Some("artist"),
        "Groups" => Some("group"),
        "Categories" => Some("category"),
        "Parodies" => Some("parody"),
        _ => None,
    }
}

struct SearchQuery {
    text: String,
    sort: Option<String>,
}

fn norm_value(v: &str) -> String {
    // NH prefers underscores for multi-word tokens
    v.trim().replace(' ', "_")
}

fn normalize_url(u: &str) -> String {
    if u.starts_with("//") {
        format!("https:{}", u)
    } else {
        u.to_string()
    }
}

fn trimmed_element_text(element: ElementRef<'_>) -> Option<String> {
    let text = element.text().collect::<String>();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn validate_gallery(body: &str, path: &str) -> Result<()> {
    let expected_id = path
        .trim_matches('/')
        .strip_prefix("g/")
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .with_context(|| format!("invalid NHentai gallery path: {path}"))?;
    let document = Html::parse_document(body);
    let id_selector = Selector::parse("#info h3#gallery_id").unwrap();
    let title_selector = Selector::parse("#info h1.title > .pretty").unwrap();
    let displayed_id = document
        .select(&id_selector)
        .next()
        .and_then(trimmed_element_text)
        .with_context(|| format!("NHentai gallery {URL}{path}: missing gallery id"))?;
    anyhow::ensure!(
        displayed_id.trim_start_matches('#').trim() == expected_id,
        "NHentai gallery {URL}{path}: response gallery id does not match requested id"
    );
    anyhow::ensure!(
        document
            .select(&title_selector)
            .next()
            .and_then(trimmed_element_text)
            .is_some(),
        "NHentai gallery {URL}{path}: missing title"
    );
    Ok(())
}

fn parse_uploaded_timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.timestamp())
}

fn build_gallery_page_urls(
    gallery_id: &str,
    gallery: &GalleryApiResponse,
    image_server: &str,
) -> Result<Vec<String>> {
    if gallery.pages.is_empty() {
        return Err(anyhow!(
            "gallery {gallery_id} ({}) API response contains no pages",
            gallery.media_id
        ));
    }

    let image_server = image_server.trim_end_matches('/');
    if image_server.is_empty() {
        return Err(anyhow!("NHentai API returned an empty image server"));
    }

    let mut pages = gallery.pages.iter().collect::<Vec<_>>();
    pages.sort_by_key(|page| page.number);

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

impl NHentai {
    fn search_url(
        &self,
        page: i64,
        query: Option<String>,
        filters: Option<Vec<Input>>,
    ) -> Result<String> {
        let query = query.filter(|text| !text.trim().is_empty());
        let filters = filters.filter(|filters| !filters.is_empty());
        if query.is_none() && filters.is_none() {
            return Err(anyhow!("query and filters cannot be both empty"));
        }
        let text_only = filters.is_none();
        let mut request = self.query_parts(query.as_deref(), filters)?;
        // Preserve the text-only popularity sort and any explicit filter sort.
        if text_only {
            request.sort = Some("popular".to_string());
        }
        let q = encode(&request.text);
        Ok(match request.sort {
            Some(sort) => format!("{URL}/search/?q={q}&sort={sort}&page={page}"),
            None => format!("{URL}/search/?q={q}&page={page}"),
        })
    }

    fn fetch_gallery(&self, path: &str) -> Result<String> {
        let url = format!("{}{}", URL, path);
        {
            let mut cache = self
                .gallery_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(body) = cache.get(&url) {
                log::debug!("{NAME}: gallery cache hit url={url}");
                return Ok(body);
            }
        }

        let body = self
            .client
            .fetch_text(&url)
            .with_context(|| format!("NHentai gallery request failed: {url}"))?;
        // Both details and the synthetic chapter must refer to a real gallery.
        // Reject unexpected responses before they can populate the shared cache.
        validate_gallery(&body, path)?;
        let mut cache = self
            .gallery_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.insert(url, body.clone());
        Ok(body)
    }

    fn fetch_cdn_servers(&self) -> Result<Vec<String>> {
        {
            let mut cache = self
                .cdn_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(image_servers) = cache.get() {
                log::debug!("{NAME}: CDN config cache hit");
                return Ok(image_servers);
            }
        }

        let cdn_url = format!("{URL}/api/v2/cdn");
        let cdn_res = self
            .client
            .fetch_text(&cdn_url)
            .map_err(|e| anyhow!(e.to_string()))?;
        let cdn: CdnConfigResponse = parse_api_response(&cdn_res, "CDN")?;
        if cdn.image_servers.is_empty() {
            return Err(anyhow!("NHentai CDN API returned no image servers"));
        }

        let mut cache = self
            .cdn_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.insert(cdn.image_servers.clone());
        Ok(cdn.image_servers)
    }

    fn query_parts(&self, text: Option<&str>, filters: Option<Vec<Input>>) -> Result<SearchQuery> {
        let mut query: Vec<String> = text
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string)
            .into_iter()
            .collect();
        let mut sort: Option<String> = None;

        // preferences: language + global blacklist
        for pref in self.preferences.iter() {
            if LANGUAGE_SELECT.eq(pref)
                && let Input::Select { state, values, .. } = pref
                && let Some(InputType::String(lang)) = state.and_then(|i| values.get(i as usize))
                && lang != "Any"
            {
                query.push(format!("language:{}", lang.to_lowercase()));
            } else if BLACKLIST_TAG.eq(pref)
                && let Input::Text {
                    state: Some(state), ..
                } = pref
            {
                for tag in state.split(',') {
                    let t = norm_value(tag);
                    if !t.is_empty() {
                        query.push(format!("-tag:{t}"));
                    }
                }
            }
        }

        // filters
        if let Some(filters) = filters {
            for filter in filters {
                let Some(canonical) = FILTER_LIST
                    .iter()
                    .find(|known| known.name() == filter.name())
                else {
                    continue;
                };
                if !canonical.eq(&filter) {
                    return Err(anyhow!("invalid {}: unexpected input type", filter.name()));
                }
                match filter {
                    Input::Text {
                        name,
                        state: Some(state),
                        ..
                    } if name == TAG_FILTER.name() => {
                        let Some(key) = nh_field_key(&name) else {
                            continue;
                        };
                        for raw in state.split(',') {
                            let raw = raw.trim();
                            if raw.is_empty() {
                                continue;
                            }
                            let neg = raw.starts_with('-');
                            let term = norm_value(raw.trim_start_matches('-'));
                            if term.is_empty() {
                                continue;
                            }
                            if neg {
                                query.push(format!("-{key}:{term}"));
                            } else {
                                query.push(format!("{key}:{term}"));
                            }
                        }
                    }
                    Input::Text {
                        name,
                        state: Some(state),
                        ..
                    } => {
                        let Some(key) = nh_field_key(&name) else {
                            continue;
                        };
                        let term = norm_value(&state);
                        if !term.is_empty() {
                            query.push(format!("{key}:{term}"));
                        }
                    }
                    Input::Select {
                        name,
                        values,
                        state,
                        ..
                    } if name == SORT_FILTER.name() => {
                        let index = state.unwrap_or(0);
                        let selected = usize::try_from(index)
                            .ok()
                            .and_then(|index| values.get(index));
                        let Some(InputType::String(value)) = selected else {
                            return Err(anyhow!(
                                "invalid Sort: selection index {index} is not a string choice"
                            ));
                        };
                        if !matches!(
                            value.as_str(),
                            "Popular" | "Popular Week" | "Popular Today" | "Recent"
                        ) {
                            return Err(anyhow!("invalid Sort: unknown choice"));
                        }
                        sort = Some(value.replace(' ', "-").to_lowercase());
                    }
                    _ => {}
                }
            }
        }

        let q = if query.is_empty() {
            r#""""#.to_string()
        } else {
            query.join(" ")
        };
        Ok(SearchQuery { text: q, sort })
    }

    fn get_manga_list(&self, url: &str, allow_empty: bool) -> Result<Vec<MangaInfo>> {
        let res = self
            .client
            .fetch_text(url)
            .map_err(|e| anyhow!(e.to_string()))?;

        let document = Html::parse_document(&res);
        let gallery_selector =
            Selector::parse(".gallery").map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let image_selector =
            Selector::parse("a > img").map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let path_selector =
            Selector::parse("a").map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let title_selector = Selector::parse("a > .caption")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;

        let mut manga_list = vec![];
        for gallery in document.select(&gallery_selector) {
            let cover_url = gallery
                .select(&image_selector)
                .flat_map(|thumbnail| thumbnail.value().attr("src"))
                .next()
                .map(normalize_url)
                .ok_or_else(|| anyhow!("cover_url not found"))?;

            let path = gallery
                .select(&path_selector)
                .flat_map(|link| link.value().attr("href"))
                .next()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("path not found"))?;

            let title = gallery
                .select(&title_selector)
                .flat_map(|caption| caption.text().next())
                .next()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("title not found"))?;

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
        // Past the end of pagination the site serves an explicit
        // "No results found" page — a legitimate empty, not breakage.
        if !allow_empty
            && !res.trim().is_empty()
            && manga_list.is_empty()
            && !res.contains("No results found")
        {
            return Err(anyhow!("parsed 0 items from {url} — markup change?"));
        }

        Ok(manga_list)
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
        self.get_manga_list(
            &format!("{URL}/search/?q={q}&sort=popular&page={page}"),
            false,
        )
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_latest_manga page={page}");
        let request = self.query_parts(None, None)?;
        let q = encode(&request.text);
        self.get_manga_list(&format!("{URL}/search/?q={q}&page={page}"), false)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: search_manga page={page} query={query:?}");
        let url = self.search_url(page, query, filters)?;
        self.get_manga_list(&url, true)
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        log::debug!("{NAME}: get_manga_detail path={path}");
        let res = self.fetch_gallery(&path)?;

        let document = Html::parse_document(&res);
        let gallery_id_selector = Selector::parse("h3[id=\"gallery_id\"]")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let parodies_selector = Selector::parse("a[href^=\"/parody/\"] > .name")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let characters_selector = Selector::parse("a[href^=\"/character/\"] > .name")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let languages_selector = Selector::parse("a[href^=\"/language/\"] > .name")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let categories_selector = Selector::parse("a[href^=\"/category/\"] > .name")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let pages_selector = Selector::parse("a[href^=\"/search/?q=pages\"] > .name")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let thumbnail_selector = Selector::parse("#cover > a > img")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let title_selector = Selector::parse("h1.title > .pretty")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let author_selector = Selector::parse("a[href^=\"/artist/\"] > .name")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let genre_selector = Selector::parse("a[href^=\"/tag/\"] > .name")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;

        let mut description = "".to_string();
        if let Some(gallery_id) = document.select(&gallery_id_selector).next().map(|el| {
            el.text()
                .map(|id| id.to_string())
                .collect::<Vec<String>>()
                .join("")
        }) {
            description = gallery_id;
        }
        let parodies = document
            .select(&parodies_selector)
            .filter_map(trimmed_element_text)
            .collect::<Vec<String>>()
            .join(",");
        if !parodies.is_empty() {
            description = format!("{}\nParodies: {}", description, parodies);
        }
        let characters = document
            .select(&characters_selector)
            .filter_map(trimmed_element_text)
            .collect::<Vec<String>>()
            .join(",");
        if !characters.is_empty() {
            description = format!("{}\nCharacters: {}", description, characters);
        }
        let languages = document
            .select(&languages_selector)
            .filter_map(trimmed_element_text)
            .collect::<Vec<String>>()
            .join(",");
        if !languages.is_empty() {
            description = format!("{}\nLanguages: {}", description, languages);
        }
        let categories = document
            .select(&categories_selector)
            .filter_map(trimmed_element_text)
            .collect::<Vec<String>>()
            .join(",");
        if !categories.is_empty() {
            description = format!("{}\nCategories: {}", description, categories);
        }
        if let Some(pages) = document.select(&pages_selector).next().map(|el| {
            el.text()
                .map(|id| id.to_string())
                .collect::<Vec<String>>()
                .join("")
        }) {
            description = format!("{}\nPages: {}", description, pages);
        }

        let cover_url = document
            .select(&thumbnail_selector)
            .flat_map(|el| el.value().attr("src"))
            .next()
            .map(normalize_url)
            .ok_or_else(|| anyhow!("cover not found"))?;

        let title = document
            .select(&title_selector)
            .filter_map(trimmed_element_text)
            .next()
            .ok_or_else(|| anyhow!("title not found"))?;

        let author: Vec<String> = document
            .select(&author_selector)
            .filter_map(trimmed_element_text)
            .collect::<Vec<String>>();

        let genre: Vec<String> = document
            .select(&genre_selector)
            .filter_map(trimmed_element_text)
            .collect::<Vec<String>>();

        let manga = MangaInfo {
            source_id: ID,
            status: None,
            path,
            description: Some(description),
            title,
            author,
            genre,
            cover_url,
        };

        Ok(manga)
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        log::debug!("{NAME}: get_chapters path={path}");
        let res = self.fetch_gallery(&path)?;

        let document = Html::parse_document(&res);
        let scanlator_selector = Selector::parse("a[href^=\"/group/\"] > .name")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
        let uploaded_selector = Selector::parse(".tags > time")
            .map_err(|e| anyhow!("failed to parse selector: {e:?}"))?;
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

        let chapter = ChapterInfo {
            source_id: ID,
            title: "Chapter 1".to_string(),
            path,
            number: 1_f64,
            scanlator,
            uploaded: uploaded.unwrap_or(0),
        };

        Ok(vec![chapter])
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        log::debug!("{NAME}: get_pages path={path}");
        let gallery_id = path
            .trim_matches('/')
            .strip_prefix("g/")
            .filter(|id| !id.is_empty() && !id.contains('/'))
            .ok_or_else(|| anyhow!("invalid NHentai gallery path: {path}"))?;
        let api_url = format!("{URL}/api/v2/galleries/{gallery_id}");
        let gallery_res = self
            .client
            .fetch_text(&api_url)
            .map_err(|e| anyhow!(e.to_string()))?;
        let gallery: GalleryApiResponse = parse_api_response(&gallery_res, "gallery")?;

        let image_servers = self.fetch_cdn_servers()?;
        let image_server = image_servers
            .first()
            .ok_or_else(|| anyhow!("NHentai CDN API returned no image servers"))?;

        build_gallery_page_urls(gallery_id, &gallery, image_server)
    }

    fn filter_list(&self) -> Vec<Input> {
        FILTER_LIST.clone()
    }

    extension_utils::impl_direct_image_fetch!(image_client, NAME, URL);
}

#[cfg(test)]
mod test {
    use super::*;

    fn create_test_instance() -> NHentai {
        let preferences: Vec<Input> = vec![
            Input::Text {
                name: "Blacklist Tag".to_string(),
                state: Some("posession".to_string()),
            },
            Input::Select {
                name: "Language".to_string(),
                values: vec![
                    InputType::String("Any".to_string()),
                    InputType::String("English".to_string()),
                    InputType::String("Japanese".to_string()),
                    InputType::String("Chinese".to_string()),
                ],
                state: Some(1),
            },
        ];

        let mut nhentai: NHentai = NHentai::default();

        nhentai.set_preferences(preferences).unwrap();

        nhentai
    }

    #[test]
    fn search_combines_text_filters_and_preferences() {
        let source = create_test_instance();
        let text = "A&B + 日本語";
        let text_filter = |name: &str, state: &str| Input::Text {
            name: name.into(),
            state: Some(state.into()),
        };
        let query_value = |url: &str| {
            let value = url
                .split('?')
                .nth(1)
                .unwrap()
                .split('&')
                .find_map(|part| part.strip_prefix("q="))
                .unwrap();
            urlencoding::decode(value).unwrap().into_owned()
        };
        let plain = source.search_url(1, Some(text.into()), None).unwrap();
        assert!(query_value(&plain).contains(text));
        assert!(query_value(&plain).contains("language:english"));
        assert!(query_value(&plain).contains("-tag:posession"));
        assert!(plain.contains("sort=popular&"));
        assert_eq!(
            plain,
            source
                .search_url(1, Some(text.into()), Some(vec![]))
                .unwrap()
        );
        let filters = vec![
            text_filter("Tag", "romance, -big breasts, , -"),
            text_filter("Future Field", "ignored"),
            Input::Select {
                name: "Sort".into(),
                values: vec![InputType::String("Popular Week".into())],
                state: Some(0),
            },
        ];
        for query in [None, Some(" ".into()), Some(text.into())] {
            let combined = source
                .search_url(2, query.clone(), Some(filters.clone()))
                .unwrap();
            let decoded = query_value(&combined);
            assert_eq!(decoded.contains(text), query.as_deref() == Some(text));
            for term in [
                "language:english",
                "-tag:posession",
                "tag:romance",
                "-tag:big_breasts",
            ] {
                assert!(decoded.contains(term), "missing {term}: {decoded}");
            }
            assert!(!decoded.contains("ignored"));
            assert!(!decoded.split_whitespace().any(|term| term == "-tag:"));
            assert!(combined.ends_with("sort=popular-week&page=2"));
        }
        for state in [-1, 4] {
            let filters = vec![Input::Select {
                name: "Sort".into(),
                values: vec![InputType::String("Popular".into())],
                state: Some(state),
            }];
            assert!(
                source
                    .search_url(1, Some(text.into()), Some(filters))
                    .unwrap_err()
                    .to_string()
                    .contains("Sort")
            );
        }
        for filters in [None, Some(vec![])] {
            assert!(source.search_url(1, None, filters.clone()).is_err());
            assert!(source.search_url(1, Some(" ".into()), filters).is_err());
        }
    }

    #[test]
    fn gallery_page_cache_keeps_multiple_recent_entries() {
        let mut cache = GalleryPageCache::default();
        cache.insert("https://nhentai.net/g/one".to_string(), "one".to_string());
        cache.insert("https://nhentai.net/g/two".to_string(), "two".to_string());

        assert_eq!(
            cache.get("https://nhentai.net/g/one"),
            Some("one".to_string())
        );
        assert_eq!(
            cache.get("https://nhentai.net/g/two"),
            Some("two".to_string())
        );
    }

    #[test]
    fn gallery_api_page_numbers_sort_cdn_urls() {
        let gallery = GalleryApiResponse {
            media_id: "123".to_string(),
            pages: vec![
                GalleryApiPage {
                    number: 2,
                    path: "galleries/123/2.webp".to_string(),
                },
                GalleryApiPage {
                    number: 1,
                    path: "galleries/123/1.jpg".to_string(),
                },
            ],
        };

        assert_eq!(
            build_gallery_page_urls("456", &gallery, "https://i2.nhentai.net/").unwrap(),
            vec![
                "https://i2.nhentai.net/galleries/123/1.jpg",
                "https://i2.nhentai.net/galleries/123/2.webp",
            ]
        );
    }

    #[test]
    fn parse_nhentai_metadata_text_and_timestamp() {
        let document = Html::parse_document(
            r#"<span class="name"><span class="community"> </span> original </span>"#,
        );
        let selector = Selector::parse(".name").unwrap();

        assert_eq!(
            document
                .select(&selector)
                .next()
                .and_then(trimmed_element_text),
            Some("original".to_string())
        );
        assert_eq!(
            parse_uploaded_timestamp("2018-03-20T11:24:45.000Z"),
            Some(1_521_545_085)
        );
    }

    #[test]
    fn parse_flaresolverr_wrapped_api_response() {
        let response: GalleryApiResponse = parse_api_response(
            r#"<html><body><pre>{"media_id":"123","pages":[{"number":1,"path":"galleries/123/1.jpg"}]}</pre></body></html>"#,
            "gallery",
        )
        .unwrap();

        assert_eq!(response.media_id, "123");
        assert_eq!(response.pages[0].path, "galleries/123/1.jpg");
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_popular_manga() {
        let nhentai: NHentai = create_test_instance();

        let res = nhentai.get_popular_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_latest_manga() {
        std::thread::sleep(std::time::Duration::from_secs(1));

        let nhentai: NHentai = create_test_instance();

        let res = nhentai.get_latest_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_search_manga() {
        std::thread::sleep(std::time::Duration::from_secs(2));

        let nhentai: NHentai = create_test_instance();

        let res = nhentai
            .search_manga(1, Some("azur lane".to_string()), None)
            .unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_search_manga_filter() {
        std::thread::sleep(std::time::Duration::from_secs(3));

        let nhentai: NHentai = create_test_instance();

        let mut filters = nhentai.filter_list();
        for filter in filters.iter_mut() {
            if SORT_FILTER.eq(filter)
                && let Input::Select { state, .. } = filter
            {
                *state = Some(1);
            } else if TAG_FILTER.eq(filter)
                && let Input::Text { state, .. } = filter
            {
                *state = Some("-big breasts".to_string());
            } else if PARODIES_FILTER.eq(filter)
                && let Input::Text { state, .. } = filter
            {
                *state = Some("azur-lane".to_string());
            }
        }
        let res = nhentai.search_manga(1, None, Some(filters)).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_manga_detail() {
        let nhentai: NHentai = create_test_instance();

        let res = nhentai.get_manga_detail("/g/385965".to_string()).unwrap();

        assert_eq!(res.title, "Lady, Maid ni datsu");
        assert!(res.genre.iter().all(|tag| !tag.trim().is_empty()));
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_chapters() {
        std::thread::sleep(std::time::Duration::from_secs(1));

        let nhentai: NHentai = create_test_instance();

        let res = nhentai.get_chapters("/g/385965".to_string()).unwrap();
        // A gallery is a complete work exposed as exactly one synthetic chapter.
        assert_eq!(res.len(), 1, "a gallery must contain exactly one chapter");
        assert_eq!(res[0].path, "/g/385965");
        assert_eq!(res[0].number, 1.0);
        assert!(res.iter().all(|chapter| chapter.uploaded > 0));
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_pages() {
        std::thread::sleep(std::time::Duration::from_secs(2));

        let nhentai: NHentai = create_test_instance();

        let page = "/g/385965".to_string();
        let res = nhentai.get_pages(page).unwrap();
        assert!(!res.is_empty());
        assert!(res[0].starts_with("https://i"));
        assert!(res[0].ends_with("/galleries/2099700/1.jpg"));
        extension_utils::assert_valid_page_image(&nhentai, &res[0]);

        let page = "/g/624576".to_string();
        let res = nhentai.get_pages(page).unwrap();
        assert!(!res.is_empty());
        assert!(res[1].starts_with("https://i"));
        assert!(res[1].ends_with("/galleries/3748415/2.webp"));
        extension_utils::assert_valid_page_image(&nhentai, &res[1]);
    }
}
