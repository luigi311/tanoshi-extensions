mod api;
mod dto;

use crate::api::{MangaOrder, get_chapters, get_manga_detail, get_manga_list, get_pages};
use anyhow::Result;
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

const ID: i64 = 7;
const NAME: &str = "Guya";
const URL: &str = "https://guya.cubari.moe";
const ICON_URL: &str = "https://guya.moe/static/logo_small.png";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REQUESTS_PER_SECOND: f64 = 10.0;

extension_utils::export_extension!(register, Guya, NAME);

pub struct Guya {
    client: RateLimitedAgent,
}

impl Default for Guya {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(None, Some(REQUESTS_PER_SECOND)),
        }
    }
}

impl Extension for Guya {
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
        // Guya returns its complete catalog in one response. As with the other
        // sources, page numbers below one retain first-page behavior.
        if page > 1 {
            return Ok(vec![]);
        }
        get_manga_list(URL, ID, &self.client, MangaOrder::Title)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_latest_manga page={page}");
        if page > 1 {
            return Ok(vec![]);
        }
        get_manga_list(URL, ID, &self.client, MangaOrder::Latest)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: search_manga page={page} query={query:?}");
        if page > 1 {
            return Ok(vec![]);
        }
        let manga = get_manga_list(URL, ID, &self.client, MangaOrder::Title)?;

        if let Some(query) = query {
            Ok(manga
                .into_iter()
                .filter(|m| m.title.to_lowercase().contains(&query.to_lowercase()))
                .collect())
        } else {
            Ok(manga)
        }
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        log::debug!("{NAME}: get_manga_detail path={path}");
        get_manga_detail(URL, &path, ID, &self.client)
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        log::debug!("{NAME}: get_chapters path={path}");
        get_chapters(URL, &path, ID, &self.client)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        log::debug!("{NAME}: get_pages path={path}");
        get_pages(URL, &path, &self.client)
    }

    extension_utils::impl_direct_image_fetch!(client, NAME, URL);
}

#[cfg(test)]
mod test {
    use super::*;

    // Completed Kaguya-sama: 281 main chapters plus extras, 318 hosted entries.
    // https://guya.cubari.moe/api/series/Kaguya-Wants-To-Be-Confessed-To/
    const COMPLETED_MANGA_PATH: &str = "/api/series/Kaguya-Wants-To-Be-Confessed-To";
    const COMPLETED_CHAPTER_COUNT: usize = 318;

    #[test]
    #[ignore = "live source check"]
    fn test_get_popular_manga() {
        let guya = Guya::default();
        let res = guya.get_popular_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_latest_manga() {
        let guya = Guya::default();
        let res = guya.get_latest_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_search_manga() {
        let guya = Guya::default();
        let res = guya
            .search_manga(1, Some("kaguya".to_string()), None)
            .unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_manga_detail() {
        let guya = Guya::default();
        let res = guya
            .get_manga_detail("/api/series/Kaguya-Wants-To-Be-Confessed-To/".to_string())
            .unwrap();
        assert_eq!(res.title, "Kaguya-sama: Love is War");
    }

    #[test]
    #[ignore = "live source check"]
    fn test_get_chapters() {
        let guya = Guya::default();
        let res = guya.get_chapters(COMPLETED_MANGA_PATH.to_string()).unwrap();
        assert_eq!(
            res.len(),
            COMPLETED_CHAPTER_COUNT,
            "Kaguya-sama chapter count changed"
        );
        let prefix = format!("{COMPLETED_MANGA_PATH}/");
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
        let guya = Guya::default();
        let res = guya
            .get_pages("/api/series/Kaguya-Wants-To-Be-Confessed-To/1".to_string())
            .unwrap();
        assert!(!res.is_empty());
        extension_utils::assert_valid_page_image(&guya, &res[0]);
    }
}
