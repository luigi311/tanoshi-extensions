use anyhow::{Context, Result, anyhow};
use chrono::{Duration, NaiveDateTime, Utc};
use extension_utils::{description_text, element_text, optional_text, unique_text};
use networking::FetchedDocument;
use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

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
    response: &FetchedDocument,
    selector: &Selector,
) -> Result<Vec<MangaInfo>> {
    let doc = Html::parse_document(&response.body);

    let selector_name =
        Selector::parse("div.item-summary > a > h3, div.data > h3 > a, div.post-title > h3 > a")
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

            let Some(url_element) = el.select(&selector_url).next() else {
                log::warn!("Skipping malformed Madara manga card from {url}: missing URL element");
                return None;
            };
            let Some(href) = url_element.value().attr("href") else {
                log::warn!("Skipping malformed Madara manga card from {url}: missing URL href");
                return None;
            };
            let path = match extension_utils::source_link_path(url, &response.final_url, href) {
                Ok(path) => path,
                Err(error) => {
                    log::warn!(
                        "Skipping malformed Madara manga card from {}: {error:#}",
                        response.final_url
                    );
                    return None;
                }
            };

            let cover = el
                .select(&selector_img)
                .next()
                .and_then(|image| get_data_src(&image));
            let cover_url =
                extension_utils::resolve_cover_url(&response.final_url, cover.as_deref());

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

pub(crate) fn parse_detail(
    url: &str,
    path: &str,
    source_id: i64,
    response: &FetchedDocument,
) -> Result<MangaInfo> {
    let doc = Html::parse_document(&response.body);

    let manga_path = path.to_string();
    anyhow::ensure!(
        !manga_path.trim().trim_matches('/').is_empty(),
        "missing manga identity at {url}{path}"
    );

    let selector_name =
        Selector::parse(r#"div.post-title h3, div.post-title h1, div.series-title h1"#)
            .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_img = Selector::parse(".summary_image img, .series-img img")
        .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let selector_artist = Selector::parse(".author-content a, .artist-content a")
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
        author: unique_text(doc.select(&selector_artist).filter_map(element_text)),
        genre: doc
            .select(&selector_genre)
            .filter_map(element_text)
            .collect(),
        status: None,
        description: optional_text(
            doc.select(&selector_desc)
                .filter_map(description_text)
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        path: manga_path,
        cover_url: extension_utils::resolve_cover_url(
            &response.final_url,
            doc.select(&selector_img)
                .find_map(|el| get_data_src(&el))
                .as_deref(),
        ),
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
    response: &FetchedDocument,
    selector: &Selector,
    selector_chapter_name: &Selector,
    selector_chapter_time: &Selector,
    selector_chapter_url: &Selector,
    source_id: i64,
) -> Result<Vec<ChapterInfo>> {
    let doc = Html::parse_document(&response.body);
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
            let path = extension_utils::source_link_path(url, &response.final_url, chapter_url)
                .with_context(|| {
                    format!(
                        "Madara chapter row {row} from {}: invalid chapter URL",
                        response.final_url
                    )
                })?;
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

pub(crate) fn parse_html_chapters(
    url: &str,
    path: &str,
    source_id: i64,
    response: &FetchedDocument,
) -> Result<Vec<ChapterInfo>> {
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
        response,
        &selector,
        &selector_chapter_name,
        &selector_chapter_time,
        &selector_chapter_url,
        source_id,
    )
    .with_context(|| format!("Madara chapter response from {url}{path}"))
}

pub(crate) fn parse_ajax_chapters(
    url: &str,
    source_id: i64,
    chapter_name_selector: Option<&str>,
    response: &FetchedDocument,
) -> Result<Vec<ChapterInfo>> {
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
        response,
        &selector,
        &selector_chapter_name,
        &selector_chapter_time,
        &selector_chapter_url,
        source_id,
    )
}

pub(crate) fn parse_pages(
    url: &str,
    path: &str,
    response: &FetchedDocument,
) -> Result<Vec<String>> {
    let doc = Html::parse_document(&response.body);

    let selector = Selector::parse(
        r#"div.page-break img, li.blocks-gallery-item img, .reading-content img, div.theimage img"#,
    )
    .map_err(|e| anyhow!("failed to parse selector: {:?}", e))?;

    let pages = doc
        .select(&selector)
        .enumerate()
        .map(|(index, el)| {
            let page = index + 1;
            let reference = get_data_src(&el).with_context(|| {
                format!(
                    "Madara page {page} from {}: missing image source",
                    response.final_url
                )
            })?;
            extension_utils::resolve_asset_url(&response.final_url, &reference).with_context(|| {
                format!(
                    "Madara page {page} from {}: invalid image source {reference:?}",
                    response.final_url
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if pages.is_empty() {
        return Err(anyhow!(
            "parsed 0 items from {}{} — markup change?",
            url,
            path
        ));
    }

    Ok(pages)
}
