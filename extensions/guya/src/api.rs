use std::collections::HashMap;

use anyhow::{Context, Result, ensure};
use extension_utils::{optional_text, unique_text};
use networking::{FetchedDocument, RateLimitedAgent};
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
    let response = client.fetch_document(&request_url)?;
    parse_catalog(&request_url, source_id, &response, order)
}

fn parse_catalog(
    request_url: &str,
    source_id: i64,
    response: &FetchedDocument,
    order: MangaOrder,
) -> Result<Vec<MangaInfo>> {
    let results: HashMap<String, serde_json::Value> = serde_json::from_str(&response.body)
        .with_context(|| format!("invalid Guya catalog API response from {request_url}"))?;
    let matched = results.len();
    let mut results: Vec<_> = results.into_iter().filter_map(|(title, value)| {
        match serde_json::from_value::<Detail>(value) {
            Ok(detail) => Some((title, detail)),
            Err(error) => {
                log::warn!("Skipping malformed Guya catalog record {title:?} from {request_url}: {error}");
                None
            }
        }
    }).collect();
    results.sort_by(|(title_a, detail_a), (title_b, detail_b)| match order {
        MangaOrder::Title => title_a.cmp(title_b),
        MangaOrder::Latest => detail_b
            .last_updated
            .cmp(&detail_a.last_updated)
            .then_with(|| title_a.cmp(title_b)),
    });
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
                author: unique_text([detail.author, detail.artist]),
                genre: vec![],
                status: None,
                description: optional_text(detail.description),
                path: format!("/api/series/{}", detail.slug),
                cover_url: extension_utils::resolve_cover_url(
                    &response.final_url,
                    Some(&detail.cover),
                ),
            })
        })
        .collect();

    let rejected = matched - manga.len();
    if rejected > 0 {
        log::warn!("Guya catalog from {request_url}: rejected {rejected} of {matched} records");
    }
    if matched == 0 {
        return Ok(manga);
    }
    ensure_non_empty(request_url, manga)
}

pub fn get_manga_detail(
    url: &str,
    path: &str,
    source_id: i64,
    client: &RateLimitedAgent,
) -> Result<MangaInfo> {
    let request_url = extension_utils::source_request_url(url, path)?.to_string();
    let response = client.fetch_document(&request_url)?;
    let series: Series = serde_json::from_str(&response.body)
        .with_context(|| format!("invalid Guya series API response from {request_url}"))?;
    validate_series(&series, &request_url)?;
    ensure!(
        !path.trim().trim_matches('/').is_empty(),
        "Guya detail from {request_url} has no path"
    );

    Ok(MangaInfo {
        source_id,
        title: series.title.clone(),
        author: unique_text([series.author.clone(), series.artist.clone()]),
        genre: vec![],
        status: None,
        description: optional_text(series.description.clone()),
        path: path.to_string(),
        cover_url: extension_utils::resolve_cover_url(&response.final_url, Some(&series.cover)),
    })
}

pub fn get_chapters(
    url: &str,
    path: &str,
    source_id: i64,
    client: &RateLimitedAgent,
) -> Result<Vec<ChapterInfo>> {
    let request_url = extension_utils::source_request_url(url, path)?.to_string();
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
            path: {
                let path = path.split('#').next().unwrap_or(path);
                let (base, query) = path
                    .split_once('?')
                    .map_or((path, None), |(base, query)| (base, Some(query)));
                let mut chapter_path = format!(
                    "{}/{}",
                    base.trim_end_matches('/'),
                    urlencoding::encode(&number)
                );
                if let Some(query) = query {
                    chapter_path.push('?');
                    chapter_path.push_str(query);
                }
                chapter_path
            },
            number: number.parse().unwrap_or_default(),
            scanlator: series.groups.get(group).cloned().and_then(optional_text),
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
    let mut request_url = extension_utils::source_request_url(url, path)?;
    // The final segment encodes the complete source key, including delimiters
    // and literal percent sequences. Decode only after separating URL syntax.
    let chapter_path = path
        .split(['?', '#'])
        .next()
        .unwrap_or(path)
        .trim_end_matches('/');
    let (series_path, chapter_number) = chapter_path
        .rsplit_once('/')
        .ok_or_else(|| anyhow::anyhow!("invalid Guya chapter path: {path}"))?;
    let chapter_number =
        urlencoding::decode(chapter_number).context("Guya chapter key is not valid UTF-8")?;

    request_url.set_path(series_path);
    let text = client.fetch_text(request_url.as_str())?;
    let series: Series = serde_json::from_str(&text)
        .with_context(|| format!("invalid Guya page API response from {url}{series_path}"))?;
    validate_series(&series, &format!("{url}{series_path}"))?;

    let chapter = series
        .chapters
        .get(chapter_number.as_ref())
        .ok_or_else(|| {
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn special_chapter_identities_round_trip_to_their_pages() {
        use std::{
            io::{BufRead, BufReader, Write},
            net::TcpListener,
            time::{Duration, Instant},
        };
        let keys = [
            ("extra chapter", "extra%20chapter"),
            ("特別編", "%E7%89%B9%E5%88%A5%E7%B7%A8"),
            ("extra%20chapter", "extra%2520chapter"),
            ("part/one", "part%2Fone"),
            ("what?now", "what%3Fnow"),
            ("extra#end", "extra%23end"),
            ("extra%2Fchapter", "extra%252Fchapter"),
            ("1", "1"),
        ];
        let series = Series {
            slug: "example".into(),
            title: "Example".into(),
            chapters: keys
                .iter()
                .enumerate()
                .map(|(index, (key, _))| {
                    (
                        key.to_string(),
                        Chapter {
                            folder: format!("folder-{index}"),
                            groups: HashMap::from([("group".into(), vec!["page.jpg".into()])]),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            ..Default::default()
        };
        let body = serde_json::to_string(&series).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            for _ in 0..=keys.len() {
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "Guya stub timed out");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line, "GET /api/series/example?ref=list HTTP/1.1\r\n");
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                write!(
                    reader.get_mut(),
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let client = networking::build_rate_limited_ureq_agent(None, None);
        let chapters =
            get_chapters(&url, "/api/series/example?ref=list#ignored", 7, &client).unwrap();
        for (index, (_, encoded)) in keys.iter().enumerate() {
            let path = format!("/api/series/example/{encoded}?ref=list");
            let chapter = chapters
                .iter()
                .find(|chapter| chapter.path == path)
                .unwrap();
            let pages = get_pages(&url, &chapter.path, &client).unwrap();
            assert_eq!(
                pages,
                [format!(
                    "{url}/media/manga/example/chapters/folder-{index}/group/page.jpg"
                )]
            );
        }
        server.join().unwrap();
    }

    #[test]
    fn catalog_retains_valid_siblings_and_ordering() {
        let mut response = FetchedDocument {
            final_url: "https://guya.example/api/get_all_series".into(),
            body: serde_json::json!({
                "Alpha": {"slug":"alpha", "last_updated":1},
                "Beta": {"slug":"beta", "last_updated":2, "author":" Author ", "artist":"Author"},
                "Missing slug": {"last_updated":3},
                "Wrong type": {"slug":42, "last_updated":4},
                "Blank slug": {"slug":" ", "last_updated":5}
            })
            .to_string(),
        };
        let parse = |response: &FetchedDocument, order| {
            parse_catalog(&response.final_url, 7, response, order)
        };
        let alphabetical = parse(&response, MangaOrder::Title).unwrap();
        assert_eq!(
            alphabetical
                .iter()
                .map(|m| m.title.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "Beta"]
        );
        assert!(alphabetical[0].description.is_none());
        assert!(alphabetical[0].cover_url.is_empty());
        assert!(alphabetical[0].author.is_empty());
        assert_eq!(alphabetical[1].author, ["Author"]);
        let latest = parse(&response, MangaOrder::Latest).unwrap();
        assert_eq!(
            latest.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(),
            ["Beta", "Alpha"]
        );
        response.body = r#"{"Invalid":{"last_updated":1}}"#.into();
        assert!(
            parse(&response, MangaOrder::Title)
                .unwrap_err()
                .to_string()
                .contains("no valid records")
        );
        response.body = "{}".into();
        assert!(parse(&response, MangaOrder::Title).unwrap().is_empty());
    }
}
