mod dto;
mod filter;

use crate::dto::{
    Relationship, Results,
    manga::{ListOrder, Order, request},
};
use anyhow::{Context, Result, bail, ensure};
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

pub fn map_result_to_manga(data: Relationship) -> Result<MangaInfo> {
    match data {
        Relationship::Manga {
            id,
            attributes,
            relationships,
        } => {
            ensure!(!id.trim().is_empty(), "manga is missing its id");
            let attributes = attributes.context("manga is missing its attributes")?;
            let title = ["en", "ja-ro", "ja"]
                .into_iter()
                .filter_map(|language| attributes.title.get(language))
                .chain(attributes.title.values())
                .find(|title| !title.trim().is_empty())
                .context("manga is missing its title")?
                .clone();
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

            Ok(MangaInfo {
                source_id: ID,
                title,
                author,
                genre: map_tags_to_string(attributes.tags),
                status: attributes.status.map(|s| s.to_string()),
                description: attributes
                    .description
                    .get("en")
                    .cloned()
                    .map(remove_bbcode)
                    .filter(|description| !description.trim().is_empty()),
                path: format!("/manga/{}", id),
                cover_url: if file_name.trim().is_empty() {
                    String::new()
                } else {
                    format!("https://uploads.mangadex.org/covers/{}/{}", id, file_name)
                },
            })
        }
        _ => bail!("expected a manga record"),
    }
}

pub fn map_result_to_chapter(data: Relationship) -> Result<ChapterInfo> {
    match data {
        Relationship::Chapter {
            id,
            attributes,
            relationships,
        } => {
            ensure!(!id.trim().is_empty(), "chapter is missing its id");
            let attributes = attributes.context("chapter is missing its attributes")?;
            let mut scanlator = "".to_string();
            for relationship in relationships {
                if let Relationship::ScanlationGroup { attributes, .. } = relationship
                    && let Some(name) = attributes.map(|attr| attr.name)
                {
                    scanlator = name;
                }
            }

            let volume = attributes.volume;
            let number = attributes.chapter;
            let mut title = attributes.title.unwrap_or_default();

            // All three labels may be absent for a valid chapter. Keep its empty title.
            if title.is_empty() {
                if let Some(vol) = volume {
                    title = format!("Volume {}", vol);
                }
                if let Some(ch) = number.clone() {
                    title = format!("{} Chapter {}", title, ch)
                }
                title = title.trim().to_string();
            }

            Ok(ChapterInfo {
                source_id: ID,
                title,
                path: format!("/chapter/{}", id),
                number: number
                    .and_then(|chapter| chapter.parse().ok())
                    .unwrap_or_default(),
                scanlator: Some(scanlator),
                uploaded: attributes.publish_at.timestamp(),
            })
        }
        _ => bail!("expected a chapter record"),
    }
}

pub fn map_result_to_pages(data: ResultsAtHome) -> Result<Vec<String>> {
    ensure!(data.result == "ok", "At-Home API did not report success");
    let base_url = url::Url::parse(&data.base_url).context("invalid At-Home base URL")?;
    ensure!(
        matches!(base_url.scheme(), "http" | "https")
            && base_url.host_str().is_some()
            && base_url.query().is_none()
            && base_url.fragment().is_none(),
        "invalid At-Home image server URL"
    );
    ensure!(
        !data.chapter.hash.trim().is_empty(),
        "At-Home response is missing its chapter hash"
    );
    ensure!(
        !data.chapter.data.is_empty(),
        "At-Home response contains no pages"
    );
    data.chapter
        .data
        .iter()
        .enumerate()
        .map(|(index, file)| {
            ensure!(
                !file.trim().is_empty(),
                "At-Home page {} is missing its filename",
                index + 1
            );
            Ok(format!(
                "{}/data/{}/{}",
                data.base_url.trim_end_matches('/'),
                data.chapter.hash,
                file
            ))
        })
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
        let res: Results = resp
            .body_mut()
            .read_json()
            .with_context(|| format!("invalid MangaDex listing API response from {url}"))?;
        ensure!(
            res.result == "ok",
            "MangaDex listing API did not report success: {url}"
        );
        if let dto::Data::Multiple {
            data,
            offset: response_offset,
            total,
            ..
        } = res.data
        {
            let raw_count = data.len();
            ensure!(
                response_offset >= 0 && total >= 0,
                "invalid MangaDex listing counts from {url}"
            );
            let mut manga = Vec::new();
            for (index, record) in data.into_iter().enumerate() {
                match map_result_to_manga(record) {
                    Ok(info) => manga.push(info),
                    Err(error) => log::warn!(
                        "Skipping MangaDex listing record {} from {url}: {error:#}",
                        index + 1
                    ),
                }
            }
            let rejected = raw_count - manga.len();
            if rejected > 0 {
                log::warn!(
                    "MangaDex listing from {url}: rejected {rejected} of {raw_count} records"
                );
            }
            if manga.is_empty() && (raw_count > 0 || response_offset < total) {
                bail!(
                    "MangaDex listing from {url} contains no valid manga ({raw_count} records, {rejected} rejected, offset {response_offset}, total {total})"
                );
            }
            Ok(manga)
        } else {
            bail!("expected a MangaDex collection response from {url}");
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
        let query_list = request::MangaList::search(query, filters)?;

        self.get_manga_list(page, query_list)
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        log::debug!("{NAME}: get_manga_detail path={path}");
        let url = format!(
            "{}{}?includes[]=author&includes[]=artist&includes[]=cover_art",
            URL, path
        );

        let mut resp = self.client.get(&url).call()?;
        let res: Results = resp
            .body_mut()
            .read_json()
            .with_context(|| format!("invalid MangaDex detail API response from {url}"))?;
        ensure!(
            res.result == "ok",
            "MangaDex detail API did not report success: {url}"
        );
        if let dto::Data::Single { data, .. } = res.data {
            map_result_to_manga(data).with_context(|| format!("invalid MangaDex detail from {url}"))
        } else {
            bail!("expected a MangaDex entity response from {url}");
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
            let res: Results = resp
                .body_mut()
                .read_json()
                .with_context(|| format!("invalid MangaDex chapter API response from {url}"))?;
            ensure!(
                res.result == "ok",
                "MangaDex chapter API did not report success: {url}"
            );
            let dto::Data::Multiple {
                data,
                offset: response_offset,
                total,
                ..
            } = res.data
            else {
                bail!("expected a MangaDex chapter collection from {url}");
            };

            let page_len = data.len() as i64;
            saw_data |= page_len > 0;
            feed_has_results |= total > 0;
            for (index, record) in data.into_iter().enumerate() {
                chapters.push(map_result_to_chapter(record).with_context(|| {
                    format!("invalid MangaDex chapter record {} from {url}", index + 1)
                })?);
            }

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
            bail!(
                "MangaDex feed from {URL}{path}/feed contains no chapters despite reporting results"
            );
        }

        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        log::debug!("{NAME}: get_pages path={path}");
        let chapter_id = path.replace("/chapter/", "");
        let url = format!("{}/at-home/server/{}", URL, chapter_id);
        log::debug!("{NAME}: get_pages at-home url={url}");

        let mut resp = self.client_at_home.get(&url).call()?;
        let res: ResultsAtHome = resp
            .body_mut()
            .read_json()
            .with_context(|| format!("invalid MangaDex At-Home API response from {url}"))?;
        map_result_to_pages(res).with_context(|| format!("invalid MangaDex pages from {url}"))
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
    #[ignore = "live source check"]
    fn test_get_latest_manga() {
        let mangadex = Mangadex::default();

        let res = mangadex.get_latest_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_popular_manga() {
        let mangadex = Mangadex::default();

        let res = mangadex.get_popular_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_search_manga() {
        let mangadex = Mangadex::default();

        let res = mangadex
            .search_manga(1, Some("komi".to_string()), None)
            .unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_manga_detail() {
        let mangadex = Mangadex::default();

        let res = mangadex
            .get_manga_detail("/manga/a96676e5-8ae2-425e-b549-7f15dd34a6d8".to_string())
            .unwrap();
        assert_eq!(res.title, "Komi-san wa Komyushou Desu.");
    }

    #[test]
    #[ignore = "live source check"]
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
    #[ignore = "live source check"]
    fn test_get_pages() {
        let mangadex = Mangadex::default();

        let res = mangadex
            .get_pages("/chapter/54b81138-ce88-408c-8e5a-1b301ed68d8d".to_string())
            .unwrap();

        assert!(!res.is_empty());
        extension_utils::assert_valid_page_image(&mangadex, &res[0]);
    }

    #[test]
    #[ignore = "live source check"]
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
