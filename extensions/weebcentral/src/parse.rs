use crate::{ID, URL};
use anyhow::{Context, Result};
use chrono::prelude::*;
use extension_utils::{description_text, element_text, unique_text};
use networking::FetchedDocument;
use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

fn parse_upload_timestamp(upload: &str) -> i64 {
    upload
        .parse::<DateTime<Utc>>()
        .map(|date| date.timestamp())
        .unwrap_or(0)
}

pub(super) fn parse_chapter_number(title: &str) -> Option<f64> {
    let lower = title.to_ascii_lowercase();
    let start = lower.find("chapter")? + "chapter".len();
    let number = title[start..]
        .trim_start()
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .next()?
        .trim_end_matches('.');

    number.parse().ok()
}

pub(super) fn source_id(href: &str, marker: &str, page_url: &str) -> Result<String> {
    let path = extension_utils::source_link_path(URL, page_url, href)?;
    let mut segments = path
        .split('?')
        .next()
        .unwrap_or_default()
        .trim_start_matches('/')
        .split('/');
    anyhow::ensure!(
        segments.next() == Some(marker),
        "expected /{marker}/ identity path"
    );
    let id = segments
        .next()
        .filter(|id| !id.trim().is_empty())
        .with_context(|| format!("missing {marker} id"))?;
    Ok(id.to_string())
}

const GENRE_LABELS: &[&str] = &["Tag(s)", "Tags(s)", "Genre(s)", "Genres"];

fn find_labeled_section<'a>(
    mut sections: impl Iterator<Item = ElementRef<'a>>,
    label_selector: &Selector,
    labels: &[&str],
) -> Option<ElementRef<'a>> {
    sections.find(|section| {
        section
            .select(label_selector)
            .next()
            .map(|label_element| label_element.text().collect::<String>())
            .is_some_and(|section_label| {
                labels.contains(&section_label.trim().trim_end_matches(':').trim())
            })
    })
}

pub(super) fn parse_manga_list(response: &FetchedDocument, url: &str) -> Result<Vec<MangaInfo>> {
    let mut manga_list = Vec::new();
    let document = Html::parse_document(&response.body);

    let manga_selector = Selector::parse("article.bg-base-300").unwrap();
    let title_selector = Selector::parse("div.text-ellipsis.truncate").unwrap();
    let author_selector = Selector::parse("div > span > a.link.link-info.link-hover").unwrap();
    let metadata_selector = Selector::parse("div.opacity-70").unwrap();
    let metadata_label_selector = Selector::parse("strong").unwrap();
    let metadata_value_selector = Selector::parse("span").unwrap();
    let status_selector = Selector::parse("strong + span").unwrap();
    let cover_selector = Selector::parse("picture img").unwrap();
    let url_selector = Selector::parse("a").unwrap();
    let empty_selector =
        Selector::parse("body > div[role='alert'].alert.alert-warning > span").unwrap();

    let matched = document.select(&manga_selector).count();
    for (index, manga) in document.select(&manga_selector).enumerate() {
        let card = index + 1;
        let Some(title) = manga
            .select(&title_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|title| !title.is_empty())
        else {
            log::warn!("Skipping WeebCentral manga card {card} from {url}: missing title");
            continue;
        };

        let mut authors: Vec<String> = Vec::new();
        for author in manga.select(&author_selector) {
            authors.extend(element_text(author));
        }

        let genres = find_labeled_section(
            manga.select(&metadata_selector),
            &metadata_label_selector,
            GENRE_LABELS,
        )
        .map(|section| {
            section
                .select(&metadata_value_selector)
                .filter_map(element_text)
                .collect()
        })
        .unwrap_or_default();

        let manga_id = manga
            .select(&url_selector)
            .next()
            .and_then(|el| el.value().attr("href"))
            .context("missing series link")
            .and_then(|href| source_id(href, "series", &response.final_url));
        let manga_id = match manga_id {
            Ok(id) => id,
            Err(error) => {
                log::warn!("Skipping WeebCentral manga card {card} from {url}: {error:#}");
                continue;
            }
        };

        let status = find_labeled_section(
            manga.select(&metadata_selector),
            &metadata_label_selector,
            &["Status"],
        )
        .and_then(|section| section.select(&status_selector).next())
        .and_then(element_text);
        // A missing cover does not make an otherwise usable series invalid.
        let cover_url = extension_utils::resolve_cover_url(
            &response.final_url,
            manga
                .select(&cover_selector)
                .next()
                .and_then(|el| el.value().attr("src")),
        );

        manga_list.push(MangaInfo {
            source_id: ID,
            title,
            author: unique_text(authors),
            genre: genres,
            status,
            description: None,
            path: format!("/series/{}", manga_id),
            cover_url,
        });
    }
    let rejected = matched - manga_list.len();
    if rejected > 0 {
        log::warn!("WeebCentral listing from {url}: rejected {rejected} of {matched} cards");
    }
    if manga_list.is_empty() {
        // Searches and pagination share this explicit empty-result fragment.
        let recognized_empty = matched == 0
            && document
                .select(&empty_selector)
                .any(|el| el.text().collect::<String>().trim() == "No results found");
        anyhow::ensure!(
            recognized_empty,
            "WeebCentral listing from {url}: no valid manga ({matched} matched, {rejected} rejected); unrecognized or malformed response"
        );
    }

    Ok(manga_list)
}

pub(super) fn parse_detail(response: &FetchedDocument, path: String) -> Result<MangaInfo> {
    let manga = Html::parse_document(&response.body);

    let title_selector = Selector::parse("h1.hidden.md\\:block.text-2xl.font-bold").unwrap();
    let sidebar_selector: Selector = Selector::parse("ul.flex.flex-col.gap-4 > li").unwrap();
    let label_selector = Selector::parse("strong").unwrap();
    let link_selector = Selector::parse("span > a.link.link-info.link-hover").unwrap();
    let status_selector = Selector::parse("strong + a.link.link-info.link-hover").unwrap();
    let description_selector =
        Selector::parse("ul.flex.flex-col.gap-4 > li > strong + p.whitespace-pre-wrap.break-words")
            .unwrap();
    let cover_selector = Selector::parse("picture img").unwrap();

    let title = manga
        .select(&title_selector)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|title| !title.is_empty())
        .with_context(|| format!("WeebCentral detail from {URL}{path}: missing title"))?;

    let author_sec = find_labeled_section(
        manga.select(&sidebar_selector),
        &label_selector,
        &["Author(s)"],
    );
    let genre_sec = find_labeled_section(
        manga.select(&sidebar_selector),
        &label_selector,
        GENRE_LABELS,
    );
    let status_sec = find_labeled_section(
        manga.select(&sidebar_selector),
        &label_selector,
        &["Status"],
    );

    let mut authors: Vec<String> = Vec::new();
    if let Some(author_sec) = author_sec {
        for author in author_sec.select(&link_selector) {
            authors.extend(element_text(author));
        }
    }

    let mut genres: Vec<String> = Vec::new();
    if let Some(genre_sec) = genre_sec {
        for genre in genre_sec.select(&link_selector) {
            genres.extend(element_text(genre));
        }
    }

    let status = status_sec
        .and_then(|section| section.select(&status_selector).next())
        .and_then(element_text);

    let description = manga
        .select(&description_selector)
        .next()
        .and_then(description_text);

    // Covers are optional; an empty string is the host's existing representation.
    let cover_url = extension_utils::resolve_cover_url(
        &response.final_url,
        manga
            .select(&cover_selector)
            .next()
            .and_then(|el| el.value().attr("src")),
    );

    Ok(MangaInfo {
        source_id: ID,
        title,
        author: unique_text(authors),
        genre: genres,
        status,
        description,
        path,
        cover_url,
    })
}

pub(super) fn parse_chapters(response: &FetchedDocument, path: &str) -> Result<Vec<ChapterInfo>> {
    let document = Html::parse_document(&response.body);

    let chapter_selector = Selector::parse("body > div.flex.items-center").unwrap();
    let time_selector = Selector::parse("a > time.text-datetime.opacity-50").unwrap();
    let link_selector = Selector::parse("a").unwrap();
    let title_selector = Selector::parse("a > span.grow.flex.items-center.gap-2 > span").unwrap();

    let chapter_count = document.select(&chapter_selector).count();
    let mut chapters = vec![];

    for (index, chapter) in document.select(&chapter_selector).enumerate() {
        let row = index + 1;
        let title = chapter
        .select(&title_selector)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|title| !title.is_empty())
        .with_context(|| {
            format!("WeebCentral chapter row {row} from {URL}{path}/full-chapter-list: missing title")
        })?;
        let fallback_number = chapter_count.saturating_sub(index) as f64;

        let chapter_id = chapter
        .select(&link_selector)
        .next()
        .and_then(|el| el.value().attr("href"))
        .context("missing chapter link")
        .and_then(|href| source_id(href, "chapters", &response.final_url))
        .with_context(|| {
            format!("WeebCentral chapter row {row} from {URL}{path}/full-chapter-list: missing chapter id")
        })?;

        let upload = chapter
            .select(&time_selector)
            .next()
            .and_then(element_text)
            .unwrap_or_default();

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

pub(super) fn parse_pages(response: &FetchedDocument, path: &str) -> Result<Vec<String>> {
    let document = Html::parse_document(&response.body);

    let mut panels = vec![];

    let section_selector = Selector::parse("section.w-full.pb-4.cursor-pointer").unwrap();
    let panel_selector = Selector::parse(":scope > img.mx-auto").unwrap();

    for (index, section) in document.select(&section_selector).enumerate() {
        let row = index + 1;
        let mut images = section.select(&panel_selector).peekable();
        anyhow::ensure!(
            images.peek().is_some(),
            "WeebCentral page container {row} from {URL}{path}/images: missing image"
        );
        for panel in images {
            let page = panels.len() + 1;
            let src = panel
                .value()
                .attr("src")
                .map(str::trim)
                .filter(|src| !src.is_empty())
                .with_context(|| {
                    format!("WeebCentral page {page} from {URL}{path}/images: missing image src")
                })?;
            panels.push(
                extension_utils::resolve_asset_url(&response.final_url, src).with_context(
                    || {
                        format!(
                            "WeebCentral page {page} from {}: invalid image source {src:?}",
                            response.final_url
                        )
                    },
                )?,
            );
        }
    }

    if panels.is_empty() {
        return Err(anyhow::anyhow!(
            "parsed 0 items from {URL}{path}/images — markup change?"
        ));
    }

    Ok(panels)
}
