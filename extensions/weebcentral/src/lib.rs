use anyhow::Result;
use lazy_static::lazy_static;
use networking::{build_ureq_agent, Agent};
use std::env;
use tanoshi_lib::extensions::PluginRegistrar;
use tanoshi_lib::prelude::{Extension, Input, Lang, SourceInfo, MangaInfo, ChapterInfo};
use scraper::{Html, Selector};
use urlencoding::encode;
use chrono::prelude::*;
use std::io::Read;

tanoshi_lib::export_plugin!(register);

fn register(registrar: &mut dyn PluginRegistrar) {
    registrar.register_function(Box::new(Weebcentral::default()));
}


lazy_static! {
    static ref PREFERENCES: Vec<Input> = vec![];
}

const ID: i64 = 28;
const NAME: &str = "WeebCentral";
const URL: &str = "https://weebcentral.com";

pub struct Weebcentral {
    preferences: Vec<Input>,
    client: Agent,
}

impl Default for Weebcentral {
    fn default() -> Self {
        Self {
            preferences: PREFERENCES.clone(),
            client: build_ureq_agent(None, None),
        }
    }
}

fn get_manga_list(mut page: i64, suburl: &str, client: &Agent) -> Result<Vec<tanoshi_lib::prelude::MangaInfo>> {

        if page < 1 {
            page = 1;
        }
        let offset = (page - 1) * 32;
        
        let mut manga_list = Vec::new();
        let url = format!("{}{}{}", URL, suburl, offset);
        let body = client.get(&url)
            .call()?
            .into_string()?;
        let document = Html::parse_document(&body);
            
        let manga_selector = Selector::parse("article.bg-base-300").unwrap();
        let title_selector = Selector::parse("div.text-ellipsis.truncate").unwrap();
        let author_selector = Selector::parse("div > span > a.link.link-info.link-hover").unwrap(); 
        let genre_selector = Selector::parse("div.opacity-70 > span").unwrap(); 
        let status_selector = Selector::parse("strong + span").unwrap();
        let cover_selector = Selector::parse("picture img").unwrap();
        let url_selector = Selector::parse("a").unwrap(); 
    
        for manga in document.select(&manga_selector) {
            let title = manga.select(&title_selector).next()
                .map_or_else(|| "Unknown Title".to_string(), |el| el.inner_html().trim().to_string());
            
            let mut authors: Vec<String> = Vec::new();
            for author in manga.select(&author_selector) {
                authors.push(author.inner_html().trim().to_string());
            }

            let mut genres: Vec<String> = Vec::new();
            let mut i = 0;
            for genre in manga.select(&genre_selector) {
                if i < 3 {
                    i += 1;
                    continue;
                }
                genres.push(genre.inner_html().trim().to_string());
            }
                
            let manga_url = manga.select(&url_selector).next()
                .map_or_else(|| "".to_string(), |el| el.value().attr("href").unwrap_or("").to_string());
    
            let manga_id = manga_url.split('/').nth(4).unwrap_or("").to_string(); 
    
            let status = manga.select(&status_selector).nth(1) 
                .map_or_else(|| "".to_string(), |el| el.inner_html().trim().to_string());
            let cover_url = manga.select(&cover_selector).next()
                .map_or_else(|| "".to_string(), |el| el.value().attr("src").unwrap_or("").to_string());

            manga_list.push(MangaInfo {
                source_id: ID, 
                title: title,
                author: authors,
                genre: genres,
                status: Some(status),
                description: None, 
                path: format!("/series/{}", manga_id), 
                cover_url,
            });
        }
        Ok(manga_list)
}   

impl Extension for Weebcentral {
    fn set_preferences(&mut self, preferences: Vec<Input>) -> Result<()> {
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
            icon: "https://weebcentral.com/static/images/144.png",
            languages: Lang::Single("en".to_string()),
            nsfw: false,
        }
    }


    fn get_popular_manga(&self, page: i64) -> Result<Vec<tanoshi_lib::prelude::MangaInfo>> {
        get_manga_list(page, "/search/data?limit=32&author=&text=&sort=Popularity&order=Descending&official=Any&anime=Any&adult=Any&display_mode=Full%20Display&offset=", &self.client)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<tanoshi_lib::prelude::MangaInfo>> {
        get_manga_list(page, "/search/data?limit=32&sort=Latest+Updates&order=Descending&official=Any&anime=Any&adult=Any&display_mode=Full+Display&offset=", &self.client)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _: Option<Vec<Input>>,
    ) -> Result<Vec<tanoshi_lib::prelude::MangaInfo>> {
        //TODO: Add filters
       get_manga_list(page, &format!("/search/data?author=&text={}&sort=Latest%20Updates&order=Descending&official=Any&anime=Any&adult=Any&display_mode=Full%20Display&offset=", encode(query.unwrap_or_default().as_str()).into_owned()), &self.client)
    }

    fn get_manga_detail(&self, path: String) -> Result<tanoshi_lib::prelude::MangaInfo> {
        let body = self.client.get(&format!("{URL}{path}"))
        .call()?
        .into_string()?;
    
        let manga = Html::parse_document(&body);
            
        let title_selector = Selector::parse("h1.hidden.md\\:block.text-2xl.font-bold").unwrap();
        let sidebar_selector: Selector = Selector::parse("ul.flex.flex-col.gap-4 > li").unwrap(); 
        let link_selector = Selector::parse("span > a.link.link-info.link-hover").unwrap(); 
        let status_selector = Selector::parse("strong + a.link.link-info.link-hover").unwrap();
        let description_selector = Selector::parse("ul.flex.flex-col.gap-4 > li > strong + p.whitespace-pre-wrap.break-words").unwrap();
        let cover_selector = Selector::parse("picture img").unwrap();
    
    
        let title = manga.select(&title_selector).next()
            .map_or_else(|| "Unknown Title".to_string(), |el| el.inner_html().trim().to_string());
            
            let author_sec = manga.select(&sidebar_selector).nth(0).unwrap();
            let genre_sec = manga.select(&sidebar_selector).nth(1).unwrap();
            let status_sec = manga.select(&sidebar_selector).nth(3).unwrap();
    
            let mut authors: Vec<String> = Vec::new();
            for author in author_sec.select(&link_selector) {
                authors.push(author.inner_html().trim().to_string());
            }
            
    
        let mut genres: Vec<String> = Vec::new();
        for genre in genre_sec.select(&link_selector) {
            genres.push(genre.inner_html().trim().to_string());
        }
    
        let status = status_sec.select(&status_selector).next()
            .map_or_else(|| "".to_string(), |el| el.inner_html().trim().to_string());
        
    
            
        let description = manga.select(&description_selector).next()
            .map_or_else(|| "".to_string(), |el| el.inner_html().trim().to_string());
    
    
        let cover_url = manga.select(&cover_selector).next()
            .map_or_else(|| "".to_string(), |el| el.value().attr("src").unwrap_or("").to_string());
            
    
        Ok(MangaInfo {
            source_id: ID,
            title: title,
            author: authors,
            genre: genres,
            status: Some(status),
            description: Some(description),
            path: path, 
            cover_url,
        })
    }

    fn get_chapters(&self, path: String) -> Result<Vec<tanoshi_lib::prelude::ChapterInfo>> {
        let response = self.client.get(&format!("{URL}{path}/full-chapter-list")).call()?;

        let mut reader = response.into_reader();
        let mut body = String::with_capacity(1024 * 1024);
        reader.read_to_string(&mut body)?;

        let document = Html::parse_document(&body);
            
        let chapter_selector = Selector::parse("body > div.flex.items-center").unwrap();
        let time_selector = Selector::parse("a > time.text-datetime.opacity-50").unwrap();
        let link_selector = Selector::parse("a").unwrap();
        let title_selector = Selector::parse("a > span.grow.flex.items-center.gap-2 > span").unwrap();
        
        let chapter_count = document.select(&chapter_selector).count();
        let mut chapters = vec![];
        let mut number = chapter_count.clone();

        for chapter in document.select(&chapter_selector) {
            let title = chapter.select(&title_selector).next()
                .map_or_else(|| "Unknown Title".to_string(), |el| el.inner_html().trim().to_string());

            let link = chapter.select(&link_selector).next()
                .map_or_else(|| "".to_string(), |el| el.value().attr("href").unwrap_or("").to_string());

            let upload = chapter.select(&time_selector).next()
                .map_or_else(|| "".to_string(), |el| el.inner_html().trim().to_string());

            
            chapters.push(ChapterInfo {
                source_id: ID,
                title: title.clone(),
                path: format!("/chapters/{}",link.clone().split('/').nth(4).unwrap_or("").to_string()),
                number: number as f64,
                scanlator: None,
                uploaded: upload.parse::<DateTime<Utc>>().unwrap().timestamp(),
            });
            number -= 1;
        }

        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {

        let response = self.client.get(&format!("{URL}{path}/images?is_prev=False&current_page=1&reading_style=single_page")).call()?;

        let mut reader = response.into_reader();
        let mut body = String::with_capacity(1024 * 1024); 
        reader.read_to_string(&mut body)?;

        let document = Html::parse_document(&body);

        let mut panels = vec![];
            
        let panel_selector = Selector::parse("section.w-full.pb-4.cursor-pointer > img.mx-auto").unwrap();

        for panel in document.select(&panel_selector) {
            panels.push(panel.value().attr("src").unwrap_or("").to_string());
        }
        
        Ok(panels)
    }

}
