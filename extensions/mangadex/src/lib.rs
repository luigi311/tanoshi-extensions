mod api;
mod dto;
mod filter;
mod mapping;
mod query;

use anyhow::Result;
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use query::{ListOrder, Order};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

extension_utils::export_extension!(register, Mangadex, NAME);

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
    client: RateLimitedAgent,
    client_at_home: RateLimitedAgent,
}

impl Default for Mangadex {
    fn default() -> Self {
        Self {
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

impl Extension for Mangadex {
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
        let query = query::MangaList {
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
            query::MangaList {
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
        let query_list = query::MangaList::search(query, filters)?;

        self.get_manga_list(page, query_list)
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        self.fetch_detail(path)
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        self.fetch_chapters(path)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        self.fetch_pages(path)
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
