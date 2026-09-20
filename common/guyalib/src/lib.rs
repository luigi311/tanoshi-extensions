mod dto;
use std::collections::HashMap;

use anyhow::{Context, Result, ensure};
use networking::RateLimitedAgent;
use tanoshi_lib::prelude::*;

use crate::dto::{Chapter, Detail, Series};

fn first_group_key(groups: &HashMap<String, Vec<String>>) -> Option<&str> {
    groups.keys().map(String::as_str).min()
}

fn ensure_non_empty<T>(url: &str, items: Vec<T>) -> Result<Vec<T>> {
    ensure!(
        !items.is_empty(),
        "Guya API response from {url} contains no valid records"
    );
    Ok(items)
}

fn validate_series(series: &Series, url: &str) -> Result<()> {
    ensure!(
        !series.title.trim().is_empty(),
        "Guya series from {url} is missing its title"
    );
    ensure!(
        !series.slug.trim().is_empty(),
        "Guya series from {url} is missing its slug"
    );
    Ok(())
}

fn selected_pages<'a>(chapter: &'a Chapter, context: &str) -> Result<(&'a str, &'a [String])> {
    ensure!(
        !chapter.folder.trim().is_empty(),
        "Guya chapter {context} is missing its image folder"
    );
    let group = first_group_key(&chapter.groups)
        .filter(|group| !group.trim().is_empty())
        .with_context(|| format!("Guya chapter {context} has no valid scanlation group"))?;
    let pages = &chapter.groups[group];
    ensure!(
        !pages.is_empty(),
        "Guya chapter {context} has no pages for group {group}"
    );
    for (index, page) in pages.iter().enumerate() {
        ensure!(
            !page.trim().is_empty(),
            "Guya chapter {context}, group {group}, page {}: missing filename",
            index + 1
        );
    }
    Ok((group, pages))
}

pub enum MangaOrder {
    Title,
    Latest,
}

pub fn get_manga_list(
    url: &str,
    source_id: i64,
    client: &RateLimitedAgent,
    order: MangaOrder,
) -> Result<Vec<MangaInfo>> {
    let request_url = format!("{}/api/get_all_series", url);
    let text = client.fetch_text(&request_url)?;
    let results: HashMap<String, Detail> = serde_json::from_str(&text)
        .with_context(|| format!("invalid Guya catalog API response from {request_url}"))?;

    let mut results: Vec<_> = results.into_iter().collect();
    results.sort_by(|(title_a, detail_a), (title_b, detail_b)| match order {
        MangaOrder::Title => title_a.cmp(title_b),
        MangaOrder::Latest => detail_b
            .last_updated
            .cmp(&detail_a.last_updated)
            .then_with(|| title_a.cmp(title_b)),
    });
    let matched = results.len();
    let manga: Vec<MangaInfo> = results
        .into_iter()
        .filter_map(|(title, detail)| {
            if title.trim().is_empty() || detail.slug.trim().is_empty() {
                log::warn!(
                    "Skipping Guya catalog record from {request_url}: missing title or slug"
                );
                return None;
            }
            Some(MangaInfo {
                source_id,
                title,
                author: vec![detail.author, detail.artist],
                genre: vec![],
                status: None,
                description: Some(detail.description),
                path: format!("/api/series/{}", detail.slug),
                cover_url: if detail.cover.trim().is_empty() {
                    String::new()
                } else {
                    format!("{}{}", url, detail.cover)
                },
            })
        })
        .collect();

    let rejected = matched - manga.len();
    if rejected > 0 {
        log::warn!("Guya catalog from {request_url}: rejected {rejected} of {matched} records");
    }
    ensure_non_empty(&request_url, manga)
}

pub fn get_manga_detail(
    url: &str,
    path: &str,
    source_id: i64,
    client: &RateLimitedAgent,
) -> Result<MangaInfo> {
    let request_url = format!("{url}{path}");
    let text = client.fetch_text(&request_url)?;
    let series: Series = serde_json::from_str(&text)
        .with_context(|| format!("invalid Guya series API response from {request_url}"))?;
    validate_series(&series, &request_url)?;
    ensure!(
        !path.trim().trim_matches('/').is_empty(),
        "Guya detail from {request_url} has no path"
    );

    Ok(MangaInfo {
        source_id,
        title: series.title.clone(),
        author: vec![series.author.clone(), series.artist.clone()],
        genre: vec![],
        status: None,
        description: Some(series.description.clone()),
        path: path.to_string(),
        cover_url: if series.cover.trim().is_empty() {
            String::new()
        } else {
            format!("{}{}", url, series.cover)
        },
    })
}

pub fn get_chapters(
    url: &str,
    path: &str,
    source_id: i64,
    client: &RateLimitedAgent,
) -> Result<Vec<ChapterInfo>> {
    let request_url = format!("{}{}", url, path);
    let text = client.fetch_text(&request_url)?;
    let series: Series = serde_json::from_str(&text)
        .with_context(|| format!("invalid Guya chapter API response from {request_url}"))?;
    validate_series(&series, &request_url)?;

    let mut chapters = vec![];
    for (number, chapter) in series.chapters {
        ensure!(
            !number.trim().is_empty(),
            "Guya chapter from {request_url} has no identity"
        );
        let (group, _) = selected_pages(&chapter, &format!("{request_url}/{number}"))?;
        chapters.push(ChapterInfo {
            source_id,
            title: chapter.title.clone(),
            path: format!("{}/{}", path.trim_end_matches('/'), number),
            number: number.parse().unwrap_or_default(),
            scanlator: series.groups.get(group).cloned(),
            uploaded: chapter.release_date.get(group).copied().unwrap_or_default() as i64,
        })
    }

    // Keep specials and distinct paths even when they share a numeric value.
    chapters.sort_by(|a, b| {
        a.number
            .total_cmp(&b.number)
            .then_with(|| a.path.cmp(&b.path))
    });
    ensure_non_empty(&request_url, chapters)
}

pub fn get_pages(url: &str, path: &str, client: &RateLimitedAgent) -> Result<Vec<String>> {
    let path = path.trim_end_matches('/');
    let (series_path, chapter_number) = path
        .rsplit_once('/')
        .ok_or_else(|| anyhow::anyhow!("invalid Guya chapter path: {path}"))?;

    let text = client.fetch_text(&format!("{}{}", url, series_path))?;
    let series: Series = serde_json::from_str(&text)
        .with_context(|| format!("invalid Guya page API response from {url}{series_path}"))?;
    validate_series(&series, &format!("{url}{series_path}"))?;

    let chapter = series.chapters.get(chapter_number).ok_or_else(|| {
        anyhow::anyhow!("chapter {chapter_number} not found in series {series_path}")
    })?;
    let (group, pages) = selected_pages(chapter, &format!("{url}{path}"))?;

    Ok(pages
        .iter()
        .map(|page| {
            format!(
                "{}/media/manga/{}/chapters/{}/{}/{}",
                url, series.slug, chapter.folder, group, page
            )
        })
        .collect())
}
