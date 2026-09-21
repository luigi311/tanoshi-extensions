use anyhow::{Context, Result, anyhow, bail};
use madara::{get_chapters_old, get_manga_detail, parse_manga_list, search_manga_old};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::{Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

extension_utils::export_extension!(register, Manhwa18cc, NAME);

const ID: i64 = 8;
const NAME: &str = "Manhwa18cc";
const URL: &str = "https://manhwa18.cc";
const ICON_URL: &str = "https://manhwa18.cc/images/favicon-160x160.png";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REQUESTS_PER_SECOND: f64 = 10.0;

pub struct Manhwa18cc {
    client: RateLimitedAgent,
}

impl Default for Manhwa18cc {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(None, Some(REQUESTS_PER_SECOND)),
        }
    }
}

fn get_manga_list(page: i64, orderby: &str, client: &RateLimitedAgent) -> Result<Vec<MangaInfo>> {
    let body = client.fetch_text(&format!("{URL}/webtoons/{page}?orderby={orderby}"))?;

    let selector =
        Selector::parse(".manga-item").map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    parse_manga_list(URL, ID, &body, &selector, false)
        .with_context(|| format!("Manhwa18cc listing from {URL}/webtoons/{page}?orderby={orderby}"))
}

impl Extension for Manhwa18cc {
    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: VERSION,
            icon: ICON_URL,
            languages: Lang::Multi(vec!["en".to_string(), "ko".to_string()]),
            nsfw: true,
        }
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_popular_manga page={page}");
        get_manga_list(page, "trending", &self.client)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_latest_manga page={page}");
        get_manga_list(page, "latest", &self.client)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: search_manga page={page} query={query:?}");
        if let Some(query) = query {
            search_manga_old(URL, ID, page, &query, &self.client)
        } else {
            bail!("query can not be empty")
        }
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        log::debug!("{NAME}: get_manga_detail path={path}");
        get_manga_detail(URL, &path, ID, &self.client)
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        log::debug!("{NAME}: get_chapters path={path}");
        get_chapters_old(URL, &path, ID, &self.client)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        log::debug!("{NAME}: get_pages path={path}");
        let body = self.client.fetch_text(&format!("{}{}", URL, path))?;

        let doc = Html::parse_document(&body);

        let selector = Selector::parse(r#".read-content img"#)
            .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

        let pages: Vec<String> = doc
            .select(&selector)
            .enumerate()
            .map(|(index, el)| {
                let page = index + 1;
                el.value()
                    .attr("data-src")
                    .filter(|src| !src.trim().is_empty())
                    .or_else(|| el.value().attr("src"))
                    .map(str::trim)
                    .filter(|src| !src.is_empty())
                    .map(str::to_string)
                    .with_context(|| {
                        format!("Manhwa18cc page {page} from {URL}{path}: missing image source")
                    })
            })
            .collect::<Result<_>>()?;

        if pages.is_empty() {
            return Err(anyhow!("parsed 0 items from {URL}{path} — markup change?"));
        }

        Ok(pages)
    }

    extension_utils::impl_direct_image_fetch!(client, NAME, URL);
}

#[cfg(test)]
mod test {
    use super::*;

    // Ooh La La is listed in https://manhwa18.cc/completed; 61 main chapters
    // plus split chapters make 70 hosted entries, last updated January 2024.
    const COMPLETED_MANGA_PATH: &str = "/webtoon/ooh-la-la";
    const COMPLETED_CHAPTER_COUNT: usize = 70;

    #[test]
    #[ignore = "live source check"]
    fn test_get_latest_manga() {
        let manhwa18cc = Manhwa18cc::default();

        let res1 = manhwa18cc.get_latest_manga(1).unwrap();
        assert!(!res1.is_empty());

        let res2 = manhwa18cc.get_latest_manga(2).unwrap();
        assert!(!res2.is_empty());

        assert_ne!(
            res1[0].path, res2[0].path,
            "{} should be different than {}",
            res1[0].path, res2[0].path
        );
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_popular_manga() {
        let manhwa18cc = Manhwa18cc::default();

        let res = manhwa18cc.get_popular_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_search_manga() {
        let manhwa18cc = Manhwa18cc::default();

        let res = manhwa18cc
            .search_manga(1, Some("tutoring".to_string()), None)
            .unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_manga_detail() {
        let manhwa18cc = Manhwa18cc::default();

        let res = manhwa18cc
            .get_manga_detail("/webtoon/private-tutoring-in-these-trying-times".to_string())
            .unwrap();
        assert_eq!(res.title, "Private Tutoring in These Trying Times");
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_chapters() {
        let manhwa18cc = Manhwa18cc::default();

        let res = manhwa18cc
            .get_chapters(COMPLETED_MANGA_PATH.to_string())
            .unwrap();
        assert_eq!(
            res.len(),
            COMPLETED_CHAPTER_COUNT,
            "Ooh La La chapter count changed"
        );
        let prefix = format!("{COMPLETED_MANGA_PATH}/chapter-");
        assert!(
            res.iter().all(|chapter| chapter
                .path
                .strip_prefix(&prefix)
                .is_some_and(|id| !id.is_empty())),
            "chapter paths must include a chapter number"
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
        let manhwa18cc = Manhwa18cc::default();

        let res = manhwa18cc
            .get_pages(format!("{COMPLETED_MANGA_PATH}/chapter-1"))
            .unwrap();

        assert!(!res.is_empty());
        extension_utils::assert_valid_page_image(&manhwa18cc, &res[0]);
    }
}
