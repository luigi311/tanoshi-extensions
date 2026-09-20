use anyhow::Result;
use chrono::prelude::*;
use lazy_static::lazy_static;
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};
use urlencoding::encode;

extension_utils::export_extension!(register, Weebcentral, NAME);

lazy_static! {
    static ref PREFERENCES: Vec<Input> = vec![];
}

const ID: i64 = 28;
const NAME: &str = "WeebCentral";
const URL: &str = "https://weebcentral.com";
const ICON_URL: &str = "https://weebcentral.com/static/images/144.png";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REQUESTS_PER_SECOND: f64 = 10.0;
// Get Pages seems to have its own rate limit
const PAGES_REQUESTS_PER_SECOND: f64 = 1.0;

fn parse_upload_timestamp(upload: &str) -> i64 {
    upload
        .parse::<DateTime<Utc>>()
        .map(|date| date.timestamp())
        .unwrap_or(0)
}

fn parse_chapter_number(title: &str) -> Option<f64> {
    let lower = title.to_ascii_lowercase();
    let start = lower.find("chapter")? + "chapter".len();
    let number = title[start..]
        .trim_start()
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .next()?
        .trim_end_matches('.');

    number.parse().ok()
}

fn segment_after<'a>(href: &'a str, marker: &str) -> Option<&'a str> {
    href.split('?')
        .next()?
        .split('/')
        .skip_while(|segment| *segment != marker)
        .nth(1)
        .filter(|segment| !segment.is_empty())
}

fn find_sidebar_section<'a>(
    document: &'a Html,
    sidebar_selector: &Selector,
    label_selector: &Selector,
    label: &str,
) -> Option<ElementRef<'a>> {
    document.select(sidebar_selector).find(|section| {
        section
            .select(label_selector)
            .next()
            .map(|label_element| label_element.text().collect::<String>())
            .is_some_and(|section_label| section_label.trim().trim_end_matches(':') == label)
    })
}

pub struct Weebcentral {
    preferences: Vec<Input>,
    client: RateLimitedAgent,
    client_pages: RateLimitedAgent,
}

impl Default for Weebcentral {
    fn default() -> Self {
        Self {
            preferences: PREFERENCES.clone(),
            client: build_rate_limited_ureq_agent(None, Some(REQUESTS_PER_SECOND)),
            client_pages: build_rate_limited_ureq_agent(None, Some(PAGES_REQUESTS_PER_SECOND)),
        }
    }
}

fn get_manga_list(
    mut page: i64,
    suburl: &str,
    client: &RateLimitedAgent,
    allow_empty: bool,
) -> Result<Vec<MangaInfo>> {
    if page < 1 {
        page = 1;
    }
    let offset = (page - 1) * 32;

    let mut manga_list = Vec::new();
    let url = format!("{}{}{}", URL, suburl, offset);
    let body = client.fetch_text(&url)?;
    let document = Html::parse_document(&body);

    let manga_selector = Selector::parse("article.bg-base-300").unwrap();
    let title_selector = Selector::parse("div.text-ellipsis.truncate").unwrap();
    let author_selector = Selector::parse("div > span > a.link.link-info.link-hover").unwrap();
    let metadata_selector = Selector::parse("div.opacity-70").unwrap();
    let metadata_label_selector = Selector::parse("strong").unwrap();
    let metadata_value_selector = Selector::parse("span").unwrap();
    let status_selector = Selector::parse("strong + span").unwrap();
    let cover_selector = Selector::parse("picture img").unwrap();
    let url_selector = Selector::parse("a").unwrap();

    for manga in document.select(&manga_selector) {
        let title = manga.select(&title_selector).next().map_or_else(
            || "Unknown Title".to_string(),
            |el| el.inner_html().trim().to_string(),
        );

        let mut authors: Vec<String> = Vec::new();
        for author in manga.select(&author_selector) {
            authors.push(author.inner_html().trim().to_string());
        }

        let mut genres: Vec<String> = Vec::new();
        for section in manga.select(&metadata_selector) {
            let is_tag_section = section
                .select(&metadata_label_selector)
                .next()
                .map(|label| label.text().collect::<String>())
                .is_some_and(|label| {
                    matches!(
                        label.trim().trim_end_matches(':'),
                        "Tag(s)" | "Tags(s)" | "Genre(s)" | "Genres"
                    )
                });

            if is_tag_section {
                genres.extend(
                    section
                        .select(&metadata_value_selector)
                        .map(|genre| genre.inner_html().trim().to_string()),
                );
                break;
            }
        }

        let Some(manga_id) = manga
            .select(&url_selector)
            .next()
            .and_then(|el| el.value().attr("href"))
            .and_then(|href| segment_after(href, "series"))
        else {
            log::warn!("Skipping malformed WeebCentral manga card from {url}: missing series id");
            continue;
        };

        let status = manga
            .select(&status_selector)
            .nth(1)
            .map_or_else(|| "".to_string(), |el| el.inner_html().trim().to_string());
        let cover_url = manga.select(&cover_selector).next().map_or_else(
            || "".to_string(),
            |el| el.value().attr("src").unwrap_or("").to_string(),
        );

        manga_list.push(MangaInfo {
            source_id: ID,
            title,
            author: authors,
            genre: genres,
            status: Some(status),
            description: None,
            path: format!("/series/{}", manga_id),
            cover_url,
        });
    }
    // Past the end of pagination the site returns an explicit
    // "No results found" alert fragment — a legitimate empty, not breakage.
    if !allow_empty
        && !body.trim().is_empty()
        && manga_list.is_empty()
        && !body.contains("No results found")
    {
        return Err(anyhow::anyhow!(
            "parsed 0 items from {url} — markup change?"
        ));
    }

    Ok(manga_list)
}

impl Extension for Weebcentral {
    extension_utils::impl_preferences!(preferences);

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
        get_manga_list(
            page,
            "/search/data?limit=32&author=&text=&sort=Popularity&order=Descending&official=Any&anime=Any&adult=Any&display_mode=Full%20Display&offset=",
            &self.client,
            false,
        )
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: get_latest_manga page={page}");
        get_manga_list(
            page,
            "/search/data?limit=32&sort=Latest+Updates&order=Descending&official=Any&anime=Any&adult=Any&display_mode=Full+Display&offset=",
            &self.client,
            false,
        )
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        log::debug!("{NAME}: search_manga page={page} query={query:?}");
        //TODO: Add filters
        get_manga_list(
            page,
            &format!(
                "/search/data?limit=32&author=&text={}&sort=Latest%20Updates&order=Descending&official=Any&anime=Any&adult=Any&display_mode=Full%20Display&offset=",
                encode(query.unwrap_or_default().as_str()).into_owned()
            ),
            &self.client,
            true,
        )
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        log::debug!("{NAME}: get_manga_detail path={path}");
        let body = self.client.fetch_text(&format!("{URL}{path}"))?;

        let manga = Html::parse_document(&body);

        let title_selector = Selector::parse("h1.hidden.md\\:block.text-2xl.font-bold").unwrap();
        let sidebar_selector: Selector = Selector::parse("ul.flex.flex-col.gap-4 > li").unwrap();
        let label_selector = Selector::parse("strong").unwrap();
        let link_selector = Selector::parse("span > a.link.link-info.link-hover").unwrap();
        let status_selector = Selector::parse("strong + a.link.link-info.link-hover").unwrap();
        let description_selector = Selector::parse(
            "ul.flex.flex-col.gap-4 > li > strong + p.whitespace-pre-wrap.break-words",
        )
        .unwrap();
        let cover_selector = Selector::parse("picture img").unwrap();

        let title = manga.select(&title_selector).next().map_or_else(
            || "Unknown Title".to_string(),
            |el| el.inner_html().trim().to_string(),
        );

        let author_sec =
            find_sidebar_section(&manga, &sidebar_selector, &label_selector, "Author(s)");
        let genre_sec = find_sidebar_section(&manga, &sidebar_selector, &label_selector, "Tags(s)");
        let status_sec = find_sidebar_section(&manga, &sidebar_selector, &label_selector, "Status");

        let mut authors: Vec<String> = Vec::new();
        if let Some(author_sec) = author_sec {
            for author in author_sec.select(&link_selector) {
                authors.push(author.inner_html().trim().to_string());
            }
        }

        let mut genres: Vec<String> = Vec::new();
        if let Some(genre_sec) = genre_sec {
            for genre in genre_sec.select(&link_selector) {
                genres.push(genre.inner_html().trim().to_string());
            }
        }

        let status = status_sec
            .and_then(|section| section.select(&status_selector).next())
            .map_or_else(|| "".to_string(), |el| el.inner_html().trim().to_string());

        let description = manga
            .select(&description_selector)
            .next()
            .map_or_else(|| "".to_string(), |el| el.inner_html().trim().to_string());

        let cover_url = manga.select(&cover_selector).next().map_or_else(
            || "".to_string(),
            |el| el.value().attr("src").unwrap_or("").to_string(),
        );

        Ok(MangaInfo {
            source_id: ID,
            title,
            author: authors,
            genre: genres,
            status: Some(status),
            description: Some(description),
            path,
            cover_url,
        })
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        log::debug!("{NAME}: get_chapters path={path}");
        let body = self
            .client
            .fetch_text(&format!("{URL}{path}/full-chapter-list"))?;

        let document = Html::parse_document(&body);

        let chapter_selector = Selector::parse("body > div.flex.items-center").unwrap();
        let time_selector = Selector::parse("a > time.text-datetime.opacity-50").unwrap();
        let link_selector = Selector::parse("a").unwrap();
        let title_selector =
            Selector::parse("a > span.grow.flex.items-center.gap-2 > span").unwrap();

        let chapter_count = document.select(&chapter_selector).count();
        let mut chapters = vec![];

        for (index, chapter) in document.select(&chapter_selector).enumerate() {
            let title = chapter.select(&title_selector).next().map_or_else(
                || "Unknown Title".to_string(),
                |el| el.inner_html().trim().to_string(),
            );
            let fallback_number = chapter_count.saturating_sub(index) as f64;

            let Some(chapter_id) = chapter
                .select(&link_selector)
                .next()
                .and_then(|el| el.value().attr("href"))
                .and_then(|href| segment_after(href, "chapters"))
            else {
                log::warn!(
                    "Skipping malformed WeebCentral chapter row from {URL}{path}: missing chapter id"
                );
                continue;
            };

            let upload = chapter
                .select(&time_selector)
                .next()
                .map_or_else(|| "".to_string(), |el| el.inner_html().trim().to_string());

            chapters.push(ChapterInfo {
                source_id: ID,
                title: title.clone(),
                path: format!("/chapters/{}", chapter_id),
                number: parse_chapter_number(&title).unwrap_or(fallback_number),
                scanlator: None,
                uploaded: parse_upload_timestamp(&upload),
            });
        }

        if chapters.is_empty() {
            return Err(anyhow::anyhow!(
                "parsed 0 items from {URL}{path}/full-chapter-list — markup change?"
            ));
        }

        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        log::debug!("{NAME}: get_pages path={path}");
        let body = self.client_pages.fetch_text(&format!(
            "{URL}{path}/images?is_prev=False&current_page=1&reading_style=single_page"
        ))?;

        let document = Html::parse_document(&body);

        let mut panels = vec![];

        let panel_selector =
            Selector::parse("section.w-full.pb-4.cursor-pointer > img.mx-auto").unwrap();

        for panel in document.select(&panel_selector) {
            panels.push(panel.value().attr("src").unwrap_or("").to_string());
        }

        if panels.is_empty() {
            return Err(anyhow::anyhow!(
                "parsed 0 items from {URL}{path}/images — markup change?"
            ));
        }

        Ok(panels)
    }

    extension_utils::impl_direct_image_fetch!(client, NAME, URL);
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_chapter_number_handles_decimals_and_specials() {
        assert_eq!(parse_chapter_number("Chapter 12.5"), Some(12.5));
        assert_eq!(parse_chapter_number("chapter 7 - The Return"), Some(7.0));
        assert_eq!(parse_chapter_number("Special 1"), None);
    }

    // Planetes: completed series, so the values asserted below are stable.
    const MANGA_PATH: &str = "/series/01J76XY8K8BPR60XQNGPTEJ767";

    fn create_test_instance() -> Weebcentral {
        let preferences: Vec<Input> = vec![];

        let mut weebcentral: Weebcentral = Weebcentral::default();

        weebcentral.set_preferences(preferences).unwrap();

        weebcentral
    }

    #[test]
    fn test_get_latest_manga() {
        let weebcentral = create_test_instance();

        let res1 = weebcentral.get_latest_manga(1).unwrap();
        assert!(!res1.is_empty());

        let res2 = weebcentral.get_latest_manga(2).unwrap();
        assert!(!res2.is_empty());

        assert_ne!(
            res1[0].path, res2[0].path,
            "{} should be different than {}",
            res1[0].path, res2[0].path
        );
    }

    #[test]
    fn test_get_popular_manga() {
        let weebcentral = create_test_instance();

        let res = weebcentral.get_popular_manga(1).unwrap();
        assert!(!res.is_empty());
    }

    #[test]
    fn test_get_popular_manga_past_end_is_empty_not_error() {
        // The site's "No results found" alert past the last page is a
        // legitimate empty result, not a markup-change error.
        let weebcentral = create_test_instance();

        let res = weebcentral.get_popular_manga(99999).unwrap();
        assert!(res.is_empty());
    }

    #[test]
    fn test_search_manga() {
        let weebcentral = create_test_instance();

        let res = weebcentral
            .search_manga(1, Some("planetes".to_string()), None)
            .unwrap();

        assert!(!res.is_empty());
        assert!(
            res.iter().any(|m| m.path == MANGA_PATH),
            "search results should contain {}, got {:?}",
            MANGA_PATH,
            res.iter().map(|m| m.path.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_get_manga_detail() {
        let weebcentral = create_test_instance();

        let res = weebcentral
            .get_manga_detail(MANGA_PATH.to_string())
            .unwrap();

        assert_eq!(res.title, "Planetes");

        // The fields below come from sidebar sections located by their
        // <strong> label text, so they double as label-parsing tests.
        assert!(
            res.author.iter().any(|a| a.contains("Makoto")),
            "author should be parsed from the Author(s) section, got {:?}",
            res.author
        );
        assert!(
            res.genre.iter().any(|g| g == "Sci-fi"),
            "genre should be parsed from the Tags(s) section, got {:?}",
            res.genre
        );
        assert_eq!(
            res.status.as_deref(),
            Some("Complete"),
            "status should be parsed from the Status section"
        );
    }

    #[test]
    fn test_get_chapters() {
        let weebcentral = create_test_instance();

        let res = weebcentral.get_chapters(MANGA_PATH.to_string()).unwrap();

        assert!(!res.is_empty());
        assert!(
            res.iter()
                .all(|c| c
                    .path
                    .strip_prefix("/chapters/")
                    .is_some_and(|id| !id.is_empty())),
            "chapter paths should look like /chapters/<id> with a non-empty id, got {:?}",
            res.iter().map(|c| c.path.clone()).collect::<Vec<_>>()
        );
        let uploaded_count = res.iter().filter(|c| c.uploaded > 0).count();
        assert!(
            uploaded_count * 2 >= res.len(),
            "at least half of upload dates should parse instead of falling back to epoch 0; got {uploaded_count}/{}",
            res.len()
        );
    }

    #[test]
    fn test_get_pages() {
        let weebcentral = create_test_instance();

        let chapters = weebcentral.get_chapters(MANGA_PATH.to_string()).unwrap();
        let chapter = chapters.first().expect("chapter list should not be empty");

        let res = weebcentral.get_pages(chapter.path.clone()).unwrap();
        assert!(!res.is_empty());
        assert!(
            res.iter().all(|p| p.starts_with("http")),
            "pages should be absolute image urls, got {:?}",
            res.first()
        );
    }
}
