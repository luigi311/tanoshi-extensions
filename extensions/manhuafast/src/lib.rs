use std::env;

use anyhow::bail;
use madara::{
    get_chapters, get_latest_manga, get_manga_detail, get_pages, get_popular_manga, search_manga,
};
use tanoshi_lib::prelude::{Extension, Input, Lang, PluginRegistrar, SourceInfo};
use lazy_static::lazy_static;
use networking::{Agent, build_ureq_agent, build_flaresolverr_client};

tanoshi_lib::export_plugin!(register);

fn register(registrar: &mut dyn PluginRegistrar) {
    registrar.register_function(Box::new(ManhuaFast::default()));
}

lazy_static! {
    static ref PREFERENCES: Vec<Input> = vec![];
}

const ID: i64 = 12;
const NAME: &str = "ManhuaFast";
const URL: &str = "https://manhuafast.com";

pub struct ManhuaFast {
    preferences: Vec<Input>,
    client: Agent,
}

impl Default for ManhuaFast {
    fn default() -> Self {
        let mut instance = Self {
            preferences: PREFERENCES.clone(),
            client: build_ureq_agent(None, None),
        };

        // If flaresolverr_url is set, build the client with it
        if let Ok(flaresolverr_url) = env::var("FLARESOLVERR_URL") {
            instance.client = build_flaresolverr_client(URL, &flaresolverr_url).unwrap();
        }

        instance
    }
}

impl Extension for ManhuaFast {
    fn set_preferences(
        &mut self,
        preferences: Vec<Input>,
    ) -> anyhow::Result<()> {
        for input in preferences {
            for pref in self.preferences.iter_mut() {
                if input.eq(pref) {
                    *pref = input.clone();
                }
            }
        }

        Ok(())
    }

    fn get_preferences(&self) -> anyhow::Result<Vec<Input>> {
        Ok(self.preferences.clone())
    }

    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: env!("CARGO_PKG_VERSION"),
            icon: "https://manhuafast.com/wp-content/uploads/2021/01/cropped-Dark-Star-Emperor-Manga-193x278-1-192x192.jpg",
            languages: Lang::Single("en".to_string()),
            nsfw: false,
        }
    }

    fn get_popular_manga(&self, page: i64) -> anyhow::Result<Vec<tanoshi_lib::prelude::MangaInfo>> {
        get_popular_manga(URL, ID, page, &self.client)
    }

    fn get_latest_manga(&self, page: i64) -> anyhow::Result<Vec<tanoshi_lib::prelude::MangaInfo>> {
        get_latest_manga(URL, ID, page,  &self.client)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _: Option<Vec<Input>>,
    ) -> anyhow::Result<Vec<tanoshi_lib::prelude::MangaInfo>> {
        if let Some(query) = query {
            search_manga(URL, ID, page, &query, false,  &self.client)
        } else {
            bail!("query can not be empty")
        }
    }

    fn get_manga_detail(&self, path: String) -> anyhow::Result<tanoshi_lib::prelude::MangaInfo> {
        get_manga_detail(URL, &path, ID,  &self.client)
    }

    fn get_chapters(&self, path: String) -> anyhow::Result<Vec<tanoshi_lib::prelude::ChapterInfo>> {
        get_chapters(URL, &path, ID, None,  &self.client)
    }

    fn get_pages(&self, path: String) -> anyhow::Result<Vec<String>> {
        get_pages(URL, &path,  &self.client)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn create_test_instance() -> ManhuaFast {
        let preferences: Vec<Input> = vec![];

        let mut ManhuaFast: ManhuaFast = ManhuaFast::default();
        
        ManhuaFast.set_preferences(preferences).unwrap();

        ManhuaFast
    }

    #[test]
    fn test_get_latest_manga() {
        let manhua_fast: ManhuaFast = create_test_instance();

        let res1 = manhua_fast.get_latest_manga(1).unwrap();
        assert!(!res1.is_empty());

        let res2 = manhua_fast.get_latest_manga(2).unwrap();
        assert!(!res2.is_empty());

        assert_ne!(
            res1[0].path, res2[0].path,
            "{} should be different than {}",
            res1[0].path, res2[0].path
        );
    }

    #[test]
    fn test_get_popular_manga() {
        let manhua_fast: ManhuaFast = create_test_instance();

        let res = manhua_fast.get_popular_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    fn test_search_manga() {
        let manhua_fast: ManhuaFast = create_test_instance();

        let res = manhua_fast
            .search_manga(1, Some("the+challenger".to_string()), None)
            .unwrap();

        assert!(!res.is_empty());
    }

    #[test]
    fn test_get_manga_detail() {
        let manhua_fast: ManhuaFast = create_test_instance();

        let res = manhua_fast
            .get_manga_detail("/manga/my-apprentices-are-all-female/".to_string())
            .unwrap();
        assert_eq!(res.title, "My Apprentices are all Female Devils");
    }

    #[test]
    fn test_get_chapters() {
        let manhua_fast: ManhuaFast = create_test_instance();

        let res = manhua_fast
            .get_chapters("/manga/my-apprentices-are-all-female/".to_string())
            .unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    fn test_get_pages() {
        let manhua_fast: ManhuaFast = create_test_instance();

        let res = manhua_fast
            .get_pages("/manga/my-apprentices-are-all-female/chapter-1/".to_string())
            .unwrap();

        assert!(!res.is_empty());
    }
}
