use anyhow::{Context, Result, anyhow};
use chrono::{Duration, NaiveDateTime, Utc};
use networking::{FlareClient, RateLimitedAgent};
use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

// A trait to abstract over different HTTP clients for fetching manga details.
pub trait DetailClient {
    fn fetch_body(&self, url: &str) -> anyhow::Result<String>;
}

impl DetailClient for FlareClient {
    fn fetch_body(&self, url: &str) -> anyhow::Result<String> {
        // use your FlareClient GET path (Cloudflare-aware)
        self.get_text(url)
    }
}

impl DetailClient for RateLimitedAgent {
    fn fetch_body(&self, url: &str) -> anyhow::Result<String> {
        self.fetch_text(url)
    }
}

fn get_data_src(el: &ElementRef) -> Option<String> {
    el.value()
        .attr("data-lazy-src")
        .or_else(|| el.value().attr("data-src"))
        .or_else(|| el.value().attr("src"))
        .map(|s| s.to_string())
}

pub fn parse_manga_list(
    url: &str,
    source_id: i64,
    body: &str,
    selector: &Selector,
    is_selector_url: bool,
) -> Result<Vec<MangaInfo>> {
    let doc = Html::parse_document(body);

    let selector_name = Selector::parse(if is_selector_url {
        "div.item-summary > a > h3, div.data > h3 > a, div.post-title > h3"
    } else {
        "div.item-summary > a > h3, div.data > h3 > a, div.post-title > h3 > a"
    })
    .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_url = Selector::parse("div.data a, div.post-title a, div.item-thumb a")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_img =
        Selector::parse("img").map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let matched = doc.select(selector).count();
    let manga: Vec<MangaInfo> = doc
        .select(selector)
        .filter_map(|el| {
            let Some(title) = el
                .select(&selector_name)
                .next()
                .map(|item| item.text().collect::<String>())
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty())
            else {
                log::warn!("Skipping malformed Madara manga card from {url}: missing title");
                return None;
            };

            let path = if is_selector_url {
                let Some(href) = el.value().attr("href") else {
                    log::warn!("Skipping malformed Madara manga card from {url}: missing URL href");
                    return None;
                };
                href.replace(url, "")
            } else {
                let Some(url_element) = el.select(&selector_url).next() else {
                    log::warn!(
                        "Skipping malformed Madara manga card from {url}: missing URL element"
                    );
                    return None;
                };
                let Some(href) = url_element.value().attr("href") else {
                    log::warn!("Skipping malformed Madara manga card from {url}: missing URL href");
                    return None;
                };
                href.replace(url, "")
            };
            if path.trim().trim_matches('/').is_empty() {
                log::warn!("Skipping malformed Madara manga card from {url}: missing URL");
                return None;
            }

            let cover_url = el
                .select(&selector_img)
                .next()
                .and_then(|image| get_data_src(&image))
                .filter(|cover_url| !cover_url.trim().is_empty())
                .unwrap_or_default();

            Some(MangaInfo {
                source_id,
                title,
                author: vec![],
                genre: vec![],
                status: None,
                description: None,
                path,
                cover_url,
            })
        })
        .collect();

    let rejected = matched - manga.len();
    if rejected > 0 {
        log::warn!("Madara listing from {url}: rejected {rejected} of {matched} cards");
    }
    if manga.is_empty() {
        // Manhwa18cc's search response has an explicit empty-result paragraph.
        let empty_selector = Selector::parse(".content-manga-list > .manga-lists > p").unwrap();
        let recognized_empty = matched == 0
            && doc.select(&empty_selector).any(|el| {
                let text = el.text().collect::<String>();
                let text = text.trim();
                text.starts_with("No result for \"") && text.ends_with('"')
            });
        anyhow::ensure!(
            recognized_empty,
            "parsed 0 items from {url} — markup change?"
        );
    }

    Ok(manga)
}

pub fn get_latest_manga(
    url: &str,
    source_id: i64,
    page: i64,
    client: &FlareClient,
) -> Result<Vec<MangaInfo>> {
    let form: &[(&str, &str)] = &[
        ("action", "madara_load_more"),
        ("page", &(page - 1).to_string()),
        ("template", "madara-core/content/content-archive"),
        ("vars[orderby]", "meta_value_num"),
        ("vars[paged]", "1"),
        ("vars[posts_per_page]", "20"),
        ("vars[post_type]", "wp-manga"),
        ("vars[post_status]", "publish"),
        ("vars[meta_key]", "_latest_update"),
        ("vars[order]", "desc"),
        ("vars[sidebar]", "right"),
        ("vars[manga_archives_item_layout]", "big_thumbnail"),
        ("vars[meta_query][0][key]", "_wp_manga_chapter_type"),
        ("vars[meta_query][0][value]", "manga"),
    ];

    let body = client.post_form_text(&format!("{}/wp-admin/admin-ajax.php", url), form)?;

    let selector = Selector::parse("div.page-item-detail")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    parse_manga_list(url, source_id, &body, &selector, false)
}

pub fn get_popular_manga(
    url: &str,
    source_id: i64,
    page: i64,
    client: &FlareClient,
) -> Result<Vec<MangaInfo>> {
    let form: &[(&str, &str)] = &[
        ("action", "madara_load_more"),
        ("page", &(page - 1).to_string()),
        ("template", "madara-core/content/content-archive"),
        ("vars[orderby]", "meta_value_num"),
        ("vars[paged]", "1"),
        ("vars[posts_per_page]", "20"),
        ("vars[post_type]", "wp-manga"),
        ("vars[post_status]", "publish"),
        ("vars[meta_key]", "_wp_manga_views"),
        ("vars[order]", "desc"),
        ("vars[sidebar]", "full"),
        ("vars[manga_archives_item_layout]", "big_thumbnail"),
        ("vars[meta_query][0][key]", "_wp_manga_chapter_type"),
        ("vars[meta_query][0][value]", "manga"),
    ];

    let body = client.post_form_text(&format!("{}/wp-admin/admin-ajax.php", url), form)?;

    let selector = Selector::parse("div.page-item-detail")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    parse_manga_list(url, source_id, &body, &selector, false)
}

pub fn search_manga_old(
    url: &str,
    source_id: i64,
    page: i64,
    query: &str,
    client: &RateLimitedAgent,
) -> Result<Vec<MangaInfo>> {
    let query = urlencoding::encode(query);
    let request_url = format!("{}/search?q={}&page={}", url, query, page);
    let body = client
        .fetch_text(&request_url)
        .with_context(|| format!("Madara search request failed: {request_url}"))?;

    let selector =
        Selector::parse(".manga-item").map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    parse_manga_list(url, source_id, &body, &selector, false)
        .with_context(|| format!("Madara search response from {request_url}"))
}

pub fn search_manga(
    url: &str,
    source_id: i64,
    page: i64,
    query: &str,
    is_selector_url: bool,
    client: &FlareClient,
) -> Result<Vec<MangaInfo>> {
    let form: &[(&str, &str)] = &[
        ("action", "madara_load_more"),
        ("vars[s]", query),
        ("template", "madara-core/content/content-search"),
        ("vars[paged]", "1"),
        ("vars[template]", "archive"),
        ("vars[post_type]", "wp-manga"),
        ("vars[post_status]", "publish"),
        ("vars[sidebar]", "right"),
        ("vars[manga_archives_item_layout]", "big_thumbnail"),
        ("vars[posts_per_page]", "20"),
        ("vars[meta_query][0][key]", "_wp_manga_chapter_type"),
        ("vars[meta_query][0][value]", "manga"),
        ("page", &(page - 1).to_string()),
    ];

    let body = client.post_form_text(&format!("{}/wp-admin/admin-ajax.php", url), form)?;

    let selector = if is_selector_url {
        Selector::parse("a").map_err(|e| anyhow!("failed to parse selector: {:?}", e))?
    } else {
        Selector::parse("div.c-tabs-item__content")
            .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?
    };

    parse_manga_list(url, source_id, &body, &selector, is_selector_url)
}

pub fn get_manga_detail<C: DetailClient>(
    url: &str,
    path: &str,
    source_id: i64,
    client: &C,
) -> Result<MangaInfo> {
    let body = client.fetch_body(&format!("{}{}", url, path))?;

    let doc = Html::parse_document(&body);

    let manga_path = path.replace(url, "");
    anyhow::ensure!(
        !manga_path.trim().trim_matches('/').is_empty(),
        "missing manga identity at {url}{path}"
    );

    let selector_name =
        Selector::parse(r#"div.post-title h3, div.post-title h1, div.series-title h1"#)
            .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_img = Selector::parse(".summary_image img, .series-img img")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_artist = Selector::parse(".artist-content a")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_genre = Selector::parse(r#".genres-content a"#)
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_desc = Selector::parse("div.description-summary div.summary__content, div.summary_content div.post-content_item > h5 + div, div.summary_content div.manga-excerpt, div.summary-text p")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    // Madara title elements often carry badge spans before the title text
    // (e.g. <h1><span>18+</span> Title</h1> on manhwa18.cc), so prefer the
    // trailing text node and only fall back to collecting all descendant
    // text when the title is fully wrapped in a child element.
    let title_element = doc.select(&selector_name).next();
    let title = title_element
        .and_then(|item| item.last_child())
        .and_then(|node| node.value().as_text())
        .map(|text| text.trim().to_string())
        .filter(|title| !title.is_empty())
        .or_else(|| {
            title_element
                .map(|item| item.text().collect::<String>())
                .map(|text| text.trim().to_string())
                .filter(|title| !title.is_empty())
        })
        .ok_or_else(|| anyhow!("no title found at {url}{path}"))?;

    Ok(MangaInfo {
        source_id,
        title,
        author: doc
            .select(&selector_artist)
            .flat_map(|el| el.text())
            .map(|s| s.to_string())
            .collect(),
        genre: doc
            .select(&selector_genre)
            .flat_map(|el| el.text())
            .map(|s| s.to_string())
            .collect(),
        status: None,
        description: Some(
            doc.select(&selector_desc)
                .flat_map(|el| el.text())
                .collect::<Vec<&str>>()
                .join("")
                .trim()
                .to_string(),
        )
        .filter(|description| !description.is_empty()),
        path: manga_path,
        cover_url: doc
            .select(&selector_img)
            .find_map(|el| get_data_src(&el))
            .unwrap_or_default(),
    })
}

fn parse_chapter_time(s: &str, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Try absolute formats first
    let with_time = format!("{} 00:00", s);
    if let Ok(dt) = NaiveDateTime::parse_from_str(&with_time, "%B %d, %Y %H:%M") {
        return Some(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(&with_time, "%d %b %Y %H:%M") {
        return Some(dt);
    }

    // Try relative formats: "N <unit> ago"
    let lower = s.to_lowercase();
    let parts: Vec<&str> = lower.split_whitespace().collect();
    if parts.len() >= 3 && parts.last() == Some(&"ago") {
        // Handle "a minute ago", "an hour ago" etc.
        let n: i64 = match parts[0] {
            "a" | "an" => 1,
            other => other.parse().ok()?,
        };
        let unit = parts[1].trim_end_matches('s'); // strip plural
        let duration = match unit {
            "second" => Duration::try_seconds(n),
            "minute" | "min" => Duration::try_minutes(n),
            "hour" | "hr" => Duration::try_hours(n),
            "day" => Duration::try_days(n),
            "week" => Duration::try_weeks(n),
            "month" => Duration::try_days(n.checked_mul(30)?),
            "year" => Duration::try_days(n.checked_mul(365)?),
            _ => return None,
        }?;
        return now.checked_sub_signed(duration);
    }

    None
}

fn parse_chapters(
    url: &str,
    doc: &Html,
    selector: &Selector,
    selector_chapter_name: &Selector,
    selector_chapter_time: &Selector,
    selector_chapter_url: &Selector,
    source_id: i64,
) -> Result<Vec<ChapterInfo>> {
    let selector_chapter_title = Selector::parse("a[title]")
        .map_err(|e| anyhow!("failed to parse chapter title selector: {:?}", e))?;
    let now = Utc::now().naive_utc();

    let chapters: Vec<ChapterInfo> = doc
        .select(selector)
        .enumerate()
        .map(|(index, el)| {
            let row = index + 1;
            let chapter_name = el
                .select(selector_chapter_name)
                .flat_map(|el| el.text())
                .collect::<Vec<&str>>()
                .join("")
                .trim()
                .to_string();
            anyhow::ensure!(
                !chapter_name.is_empty(),
                "Madara chapter row {row} from {url}: missing title"
            );

            let chapter_time_el = el.select(selector_chapter_time).next();

            // Try inner text first; if empty, fall back to the title attr of a child <a>
            let raw_time = chapter_time_el
                .map(|e| {
                    let text = e.text().collect::<String>().trim().to_string();
                    if !text.is_empty() {
                        text
                    } else {
                        // Look for a child <a> with a title attribute (e.g. c-new-tag)
                        e.select(&selector_chapter_title)
                            .next()
                            .and_then(|a| a.value().attr("title"))
                            .unwrap_or("")
                            .trim()
                            .to_string()
                    }
                })
                .unwrap_or_default();

            // Missing, unrecognized and out-of-range dates use the host's unknown sentinel.
            let uploaded = parse_chapter_time(&raw_time, now)
                .map(|date| date.and_utc().timestamp())
                .unwrap_or(0);

            let chapter_url = el
                .select(selector_chapter_url)
                .next()
                .and_then(|link| link.value().attr("href"))
                .filter(|href| !href.trim().is_empty())
                .with_context(|| {
                    format!("Madara chapter row {row} from {url}: missing chapter URL")
                })?;
            let path = chapter_url.replace(url, "");
            anyhow::ensure!(
                !path.trim().trim_matches('/').is_empty(),
                "Madara chapter row {row} from {url}: missing chapter identity"
            );

            Ok(ChapterInfo {
                source_id,
                title: chapter_name.clone(),
                path,
                number: chapter_name
                    .replace("Chapter ", "")
                    .split(' ')
                    .next()
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or_default(),
                scanlator: None,
                uploaded,
            })
        })
        .collect::<Result<_>>()?;

    if chapters.is_empty() {
        return Err(anyhow!("parsed 0 items from {url} — markup change?"));
    }

    Ok(chapters)
}

pub fn get_chapters_old(
    url: &str,
    path: &str,
    source_id: i64,
    client: &RateLimitedAgent,
) -> Result<Vec<ChapterInfo>> {
    let body = client.fetch_text(&format!("{}{}", url, path))?;

    let doc = Html::parse_document(&body);

    let selector = Selector::parse(r#"#chapterlist .a-h.wleft"#)
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_chapter_name = Selector::parse(r#".chapter-name"#)
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_chapter_time = Selector::parse(r#".chapter-time"#)
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_chapter_url = Selector::parse(".chapter-name")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    parse_chapters(
        url,
        &doc,
        &selector,
        &selector_chapter_name,
        &selector_chapter_time,
        &selector_chapter_url,
        source_id,
    )
    .with_context(|| format!("Madara chapter response from {url}{path}"))
}

pub fn get_chapters(
    url: &str,
    path: &str,
    source_id: i64,
    chapter_name_selector: Option<&str>,
    client: &FlareClient,
) -> Result<Vec<ChapterInfo>> {
    let body = client.post_empty_text(
        &format!("{}{}/ajax/chapters", url, path.trim_end_matches('/')),
        &[
            ("Referer", url),
            ("Content-Length", "0"),
            ("X-Requested-With", "XMLHttpRequest"),
        ],
    )?;

    let doc = Html::parse_document(&body);

    let selector = Selector::parse("li.wp-manga-chapter,li.chapter-li")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_chapter_name = Selector::parse(chapter_name_selector.unwrap_or("a"))
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_chapter_url =
        Selector::parse("a").map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_chapter_time = Selector::parse(".chapter-release-date")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    parse_chapters(
        url,
        &doc,
        &selector,
        &selector_chapter_name,
        &selector_chapter_time,
        &selector_chapter_url,
        source_id,
    )
}

pub fn get_pages(url: &str, path: &str, client: &FlareClient) -> Result<Vec<String>> {
    let body = client.post_empty_text(&format!("{}{}", url, path), &[("Referer", url)])?;

    let doc = Html::parse_document(&body);

    let selector = Selector::parse(
        r#"div.page-break img, li.blocks-gallery-item img, .reading-content img, div.theimage img"#,
    )
    .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let pages = doc
        .select(&selector)
        .flat_map(|el| get_data_src(&el))
        .map(|p| p.trim().to_string())
        .collect::<Vec<_>>();

    if pages.is_empty() {
        return Err(anyhow!(
            "parsed 0 items from {}{} — markup change?",
            url,
            path
        ));
    }

    Ok(pages)
}

#[cfg(test)]
mod test {
    use super::*;

    struct StaticClient(&'static str);

    impl DetailClient for StaticClient {
        fn fetch_body(&self, _url: &str) -> anyhow::Result<String> {
            Ok(self.0.to_string())
        }
    }

    #[test]
    fn get_manga_detail_missing_title_returns_error() {
        let result = get_manga_detail(
            "https://example.test",
            "/missing",
            1,
            &StaticClient("<html><body>No title</body></html>"),
        );

        let error = result.expect_err("missing title should return an error");
        assert_eq!(
            error.to_string(),
            "no title found at https://example.test/missing"
        );
    }

    #[test]
    fn get_manga_detail_collects_fully_wrapped_title_text() {
        // Title entirely inside a child element: no trailing text node, so
        // the collected-text fallback must kick in.
        let result = get_manga_detail(
            "https://example.test",
            "/wrapped",
            1,
            &StaticClient(r#"<div class="post-title"><h1><span>Wrapped title</span></h1></div>"#),
        )
        .expect("wrapped title should parse");

        assert_eq!(result.title, "Wrapped title");
    }

    #[test]
    fn get_manga_detail_skips_badge_span_before_title() {
        // Regression: manhwa18.cc marks adult series with a badge span inside
        // the title element; it must not be glued onto the title.
        let result = get_manga_detail(
            "https://example.test",
            "/badged",
            1,
            &StaticClient(
                r#"<div class="post-title"><h1><span>18+</span> Private Tutoring</h1></div>"#,
            ),
        )
        .expect("badged title should parse");

        assert_eq!(result.title, "Private Tutoring");
    }

    #[test]
    fn parse_manga_list_empty_markup_returns_error() {
        let selector = Selector::parse(".card").unwrap();
        let result = parse_manga_list(
            "https://example.test",
            1,
            "<html><body>unexpected response</body></html>",
            &selector,
            false,
        );

        let error = result.expect_err("empty parsed list should return an error");
        assert_eq!(
            error.to_string(),
            "parsed 0 items from https://example.test — markup change?"
        );
    }
}
