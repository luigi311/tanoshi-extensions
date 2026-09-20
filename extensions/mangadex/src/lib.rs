mod dto;
mod filter;

use crate::dto::{
    Relationship, Results,
    manga::{ListOrder, Order, Rating, request},
};
use anyhow::{Result, anyhow, bail};
use dto::ResultsAtHome;
use fancy_regex::Regex;
use lazy_static::lazy_static;
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

extension_utils::export_extension!(register, Mangadex, NAME);

lazy_static! {
    static ref PREFERENCES: Vec<Input> = vec![];
}

const ID: i64 = 2;
const NAME: &str = "Mangadex";
const URL: &str = "https://api.mangadex.org";
const SITE_URL: &str = "https://mangadex.org";
const ICON_URL: &str = "https://mangadex.org/favicon.ico";
const VERSION: &str = env!("CARGO_PKG_VERSION");
// While api.mangadex.org has a rate limit of 5 requests per second
// The /at-home/server endpoint has a 40 requests per min limit ~= 0.66 rps
const REQUESTS_PER_SECOND: f64 = 5.0;
const REQUESTS_PER_SECOND_AT_HOME: f64 = 0.6;
const CHAPTER_PAGE_LIMIT: i64 = 500;

pub struct Mangadex {
    preferences: Vec<Input>,
    client: RateLimitedAgent,
    client_at_home: RateLimitedAgent,
}

impl Default for Mangadex {
    fn default() -> Self {
        Self {
            preferences: PREFERENCES.clone(),
            client: build_rate_limited_ureq_agent(
                Some(format!("Tanoshi-Extension/{VERSION}").as_str()),
                Some(REQUESTS_PER_SECOND),
            ),
            client_at_home: build_rate_limited_ureq_agent(
                Some(format!("Tanoshi-Extension/{VERSION}").as_str()),
                Some(REQUESTS_PER_SECOND_AT_HOME),
            ),
        }
    }
}

#[must_use]
fn remove_bbcode(string: String) -> String {
    let regex = Regex::new(r#"\[(\w+)[^]]*](.*?)\[/\1]"#).unwrap();

    let result = string
        .replace("[list]", "")
        .replace("[/list]", "")
        .replace("[*]", "")
        .replace("[hr]", "\n");

    regex.replace_all(&result, "$2").to_string()
}

pub fn map_tags_to_string(relationships: Vec<Relationship>) -> Vec<String> {
    let mut tags = vec![];
    for relationship in relationships {
        if let Relationship::Tag { attributes, .. } = relationship
            && let Some(name) = attributes.and_then(|attr| attr.name.get("en").cloned())
        {
            tags.push(name);
        }
    }

    tags
}

pub fn map_result_to_manga(data: Relationship) -> Option<MangaInfo> {
    match data {
        Relationship::Manga {
            id,
            attributes,
            relationships,
        } => {
            let mut author = vec![];
            let mut genre = vec![];
            let mut file_name = "".to_string();
            for relationship in relationships {
                match relationship {
                    Relationship::Author { attributes, .. } => {
                        if let Some(name) = attributes.map(|attr| attr.name) {
                            author.push(name);
                        }
                    }
                    Relationship::Artist { attributes, .. } => {
                        if let Some(name) = attributes.map(|attr| attr.name) {
                            author.push(name);
                        }
                    }
                    Relationship::Tag { attributes, .. } => {
                        if let Some(name) = attributes.and_then(|attr| attr.name.get("en").cloned())
                        {
                            genre.push(name.to_owned());
                        }
                    }
                    Relationship::CoverArt { attributes, .. } => {
                        if let Some(name) = attributes.map(|attr| attr.file_name) {
                            file_name = name;
                        }
                    }
                    _ => {}
                };
            }

            Some(MangaInfo {
                source_id: ID,
                title: attributes
                    .clone()
                    .and_then(|attr| {
                        if let Some(title) = attr.title.get("en").cloned() {
                            Some(title)
                        } else if let Some(title) = attr.title.get("ja-ro").cloned() {
                            Some(title)
                        } else if let Some(title) = attr.title.get("ja").cloned() {
                            Some(title)
                        } else {
                            attr.title.values().next().cloned()
                        }
                    })
                    .unwrap_or_else(String::new),
                author,
                genre: attributes
                    .clone()
                    .map(|attr| attr.tags)
                    .map(map_tags_to_string)
                    .unwrap_or_else(Vec::new),
                status: attributes
                    .clone()
                    .and_then(|attr| attr.status)
                    .map(|s| s.to_string()),
                description: attributes
                    .and_then(|attr| attr.description.get("en").cloned())
                    .map(remove_bbcode),
                path: format!("/manga/{}", id),
                cover_url: format!("https://uploads.mangadex.org/covers/{}/{}", id, file_name),
            })
        }
        _ => None,
    }
}

pub fn map_result_to_chapter(data: Relationship) -> Option<ChapterInfo> {
    match data {
        Relationship::Chapter {
            id,
            attributes,
            relationships,
        } => {
            let mut scanlator = "".to_string();
            for relationship in relationships {
                if let Relationship::ScanlationGroup { attributes, .. } = relationship
                    && let Some(name) = attributes.map(|attr| attr.name)
                {
                    scanlator = name;
                }
            }

            let volume = attributes.clone().and_then(|attr| attr.volume);
            let number = attributes.clone().and_then(|attr| attr.chapter);
            let mut title = attributes
                .clone()
                .and_then(|attr| attr.title)
                .unwrap_or_else(|| "".to_string());

            if title.is_empty() {
                if let Some(vol) = volume {
                    title = format!("Volume {}", vol);
                }
                if let Some(ch) = number.clone() {
                    title = format!("{} Chapter {}", title, ch)
                }
                title = title.trim().to_string();
            }

            Some(ChapterInfo {
                source_id: ID,
                title,
                path: format!("/chapter/{}", id),
                number: number
                    .and_then(|chapter| chapter.parse().ok())
                    .unwrap_or_default(),
                scanlator: Some(scanlator),
                uploaded: attributes
                    .map(|attr| attr.publish_at.naive_utc().and_utc().timestamp())
                    .unwrap_or_else(|| 0),
            })
        }
        _ => None,
    }
}

pub fn map_result_to_pages(data: ResultsAtHome) -> Vec<String> {
    data.chapter
        .data
        .iter()
        .map(|d| format!("{}/data/{}/{}", data.base_url, data.chapter.hash, d))
        .collect()
}

impl Mangadex {
    fn get_manga_list(&self, mut page: i64, query: request::MangaList) -> Result<Vec<MangaInfo>> {
        if page < 1 {
            page = 1;
        }
        let offset = (page - 1) * 20;
        let query = request::MangaList {
            limit: 20,
            offset,
            ..query
        };

        let url = format!("{}/manga?{}", URL, query.to_query_string()?);

        // ureq v3: read JSON from the body
        let mut resp = self.client.get(&url).call()?;
        let res: Results = resp.body_mut().read_json()?;
        if let dto::Data::Multiple {
            data,
            offset: response_offset,
            total,
            ..
        } = res.data
        {
            let raw_count = data.len();
            let manga: Vec<MangaInfo> = data.into_iter().filter_map(map_result_to_manga).collect();
            if manga.is_empty() && (raw_count > 0 || response_offset < total) {
                bail!("parsed 0 items from {url} — markup change?");
            }
            Ok(manga)
        } else {
            bail!("invalid data");
        }
    }
}

impl Extension for Mangadex {
    extension_utils::impl_preferences!(preferences);

    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: VERSION,
            icon: ICON_URL,
            languages: Lang::All,
            nsfw: true,
        }
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_popular_manga page={page}");
        let query = request::MangaList {
            order: Some(ListOrder {
                followed_count: Some(Order::Desc),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.get_manga_list(page, query)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_latest_manga page={page}");
        self.get_manga_list(
            page,
            request::MangaList {
                order: Some(ListOrder {
                    latest_uploaded_chapter: Some(Order::Desc),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: search_manga page={page} query={query:?}");
        let query_list = if let Some(filters) = filters {
            filters.into()
        } else if let Some(query) = query {
            request::MangaList {
                title: Some(query),
                content_rating: vec![
                    Rating::Safe,
                    Rating::Suggestive,
                    Rating::Erotica,
                    Rating::Pornographic,
                ],
                ..Default::default()
            }
        } else {
            bail!("query and filters cannot be both empty")
        };

        self.get_manga_list(page, query_list)
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        log::debug!("{NAME}: get_manga_detail path={path}");
        let url = format!(
            "{}{}?includes[]=author&includes[]=artist&includes[]=cover_art",
            URL, path
        );

        let mut resp = self.client.get(&url).call()?;
        let res: Results = resp.body_mut().read_json()?;
        if let dto::Data::Single { data, .. } = res.data {
            map_result_to_manga(data).ok_or_else(|| anyhow!("no such manga"))
        } else {
            bail!("invalid data");
        }
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        log::debug!("{NAME}: get_chapters path={path}");
        let mut offset = 0;
        let mut chapters = Vec::new();
        let mut feed_has_results = false;
        let mut saw_data = false;

        loop {
            let url = format!(
                "{}{}/feed?limit={CHAPTER_PAGE_LIMIT}&offset={offset}&contentRating[]=safe&contentRating[]=suggestive&contentRating[]=erotica&contentRating[]=pornographic&translatedLanguage[]=en&includes[]=scanlation_group",
                URL, path
            );

            let mut resp = self.client.get(&url).call()?;
            let res: Results = resp.body_mut().read_json()?;
            let dto::Data::Multiple {
                data,
                offset: response_offset,
                total,
                ..
            } = res.data
            else {
                bail!("invalid data");
            };

            let page_len = data.len() as i64;
            saw_data |= page_len > 0;
            feed_has_results |= total > 0;
            chapters.extend(data.into_iter().filter_map(map_result_to_chapter));

            let next_offset = response_offset + page_len;
            if page_len == 0 || next_offset >= total {
                break;
            }
            if next_offset <= offset {
                bail!("MangaDex chapter feed pagination did not advance");
            }
            offset = next_offset;
        }

        if chapters.is_empty() && (saw_data || feed_has_results) {
            bail!("parsed 0 items from {URL}{path}/feed — markup change?");
        }

        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        log::debug!("{NAME}: get_pages path={path}");
        let chapter_id = path.replace("/chapter/", "");
        let url = format!("{}/at-home/server/{}", URL, chapter_id);
        log::debug!("{NAME}: get_pages at-home url={url}");

        let mut resp = self.client_at_home.get(&url).call()?;
        let res: ResultsAtHome = resp.body_mut().read_json()?;
        let pages = map_result_to_pages(res);
        if pages.is_empty() {
            bail!("parsed 0 items from {url} — markup change?");
        }

        Ok(pages)
    }

    fn filter_list(&self) -> Vec<Input> {
        filter::FILTER_LIST.clone()
    }

    extension_utils::impl_direct_image_fetch!(client, NAME, SITE_URL);
}

#[cfg(test)]
mod test {
    use super::*;

    // Completed Kaguya-sama, verified against the English feed. Count releases,
    // including extras and multiple scanlation groups, rather than chapter numbers.
    // https://api.mangadex.org/manga/37f5cce0-8070-4ada-96e5-fa24b1bd4ff9
    const COMPLETED_MANGA_PATH: &str = "/manga/37f5cce0-8070-4ada-96e5-fa24b1bd4ff9";
    const COMPLETED_CHAPTER_COUNT: usize = 371;

    #[test]
    fn test_get_latest_manga() {
        let mangadex = Mangadex::default();

        let res = mangadex.get_latest_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    fn test_get_popular_manga() {
        let mangadex = Mangadex::default();

        let res = mangadex.get_popular_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    fn test_search_manga() {
        let mangadex = Mangadex::default();

        let res = mangadex
            .search_manga(1, Some("komi".to_string()), None)
            .unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    fn test_get_manga_detail() {
        let mangadex = Mangadex::default();

        let res = mangadex
            .get_manga_detail("/manga/a96676e5-8ae2-425e-b549-7f15dd34a6d8".to_string())
            .unwrap();
        assert_eq!(res.title, "Komi-san wa Komyushou Desu.");
    }

    #[test]
    fn test_get_chapters() {
        let mangadex = Mangadex::default();

        let res = mangadex
            .get_chapters(COMPLETED_MANGA_PATH.to_string())
            .unwrap();
        assert_eq!(
            res.len(),
            COMPLETED_CHAPTER_COUNT,
            "Kaguya-sama English release count changed"
        );
        assert!(
            res.iter().all(|chapter| chapter
                .path
                .strip_prefix("/chapter/")
                .is_some_and(|id| !id.is_empty())),
            "chapter paths must include a chapter ID"
        );
        let unique_paths: std::collections::HashSet<_> =
            res.iter().map(|chapter| &chapter.path).collect();
        assert_eq!(
            unique_paths.len(),
            res.len(),
            "chapter paths must be unique"
        );
    }

    #[test]
    fn test_get_pages() {
        let mangadex = Mangadex::default();

        let res = mangadex
            .get_pages("/chapter/54b81138-ce88-408c-8e5a-1b301ed68d8d".to_string())
            .unwrap();

        assert!(!res.is_empty());
    }

    #[test]
    fn test_large_image() {
        // Test downloading and saving a large image from Mangadex as they support pngs which can be larger than 10mb standard limits.
        // https://cmdxd98sb0x3yprd.mangadex.network/data/ffc278361423df8bab7a0fff52689f0b/24-efcc5b0ee5e24f2c1ac1f15df114dbae078cb3618792308bf55a4cec7d390ae9.png
        let mangadex = Mangadex::default();
        let url = "https://cmdxd98sb0x3yprd.mangadex.network/data/ffc278361423df8bab7a0fff52689f0b/24-efcc5b0ee5e24f2c1ac1f15df114dbae078cb3618792308bf55a4cec7d390ae9.png"
            .to_string();
        let bytes = mangadex.get_image_bytes(url).unwrap();
        assert!(!bytes.is_empty());
        assert!(bytes.len() > 10 * 1024 * 1024); // Ensure the image is larger than 10MB
    }
}
