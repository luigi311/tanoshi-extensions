use anyhow::Result;
use tanoshi_lib::extensions::PluginRegistrar;
use tanoshi_lib::prelude::{Extension, Input, Lang, SourceInfo};
use lazy_static::lazy_static;
use networking::{Agent, build_ureq_agent};
use std::env;

tanoshi_lib::export_plugin!(register);

fn register(registrar: &mut dyn PluginRegistrar) {
    registrar.register_function(Box::new(Mangasee::default()));
}

lazy_static! {
    static ref PREFERENCES: Vec<Input> = vec![];
}

const ID: i64 = 3;
const NAME: &str = "MangaSee";
const URL: &str = "https://mangasee123.com";

pub struct Mangasee {
    preferences: Vec<Input>,
    client: Agent,
}

impl Default for Mangasee {
    fn default() -> Self {
        Self {
            preferences: PREFERENCES.clone(),
            client: build_ureq_agent(None, None),
        }
    }
}

impl Extension for Mangasee {
    fn set_preferences(
        &mut self,
        preferences: Vec<Input>,
    ) -> Result<()> {
        for input in preferences {
            for pref in self.preferences.iter_mut() {
                if input.eq(pref) {
                    *pref = input.clone();
                }
            }
        }

        Ok(())
    }

    fn get_preferences(&self) -> Result<Vec<Input>> {
        Ok(self.preferences.clone())
    }

    fn get_source_info(&self) -> tanoshi_lib::prelude::SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: env!("CARGO_PKG_VERSION"),
            icon: "https://mangasee123.com/media/favicon.png",
            languages: Lang::Single("en".to_string()),
            nsfw: false,
        }
    }

    fn filter_list(&self) -> Vec<Input> {
        nepnep::get_filter_list()
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<tanoshi_lib::prelude::MangaInfo>> {
        nepnep::get_popular_manga(ID, URL, page, &self.client)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<tanoshi_lib::prelude::MangaInfo>> {
        nepnep::get_latest_manga(ID, URL, page, &self.client)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        filters: Option<Vec<Input>>,
    ) -> Result<Vec<tanoshi_lib::prelude::MangaInfo>> {
        nepnep::search_manga(ID, URL, page, query, filters, &self.client)
    }

    fn get_manga_detail(&self, path: String) -> Result<tanoshi_lib::prelude::MangaInfo> {
        nepnep::get_manga_detail(ID, URL, path, &self.client)
    }

    fn get_chapters(&self, path: String) -> Result<Vec<tanoshi_lib::prelude::ChapterInfo>> {
        nepnep::get_chapters(ID, URL, path, &self.client)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        nepnep::get_pages(URL, path, &self.client)
    }
}
