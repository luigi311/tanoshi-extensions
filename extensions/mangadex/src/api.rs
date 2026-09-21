use anyhow::{Context, Result, bail, ensure};
use std::collections::HashSet;
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::{
    CHAPTER_PAGE_LIMIT, Mangadex, NAME, URL,
    dto::{
        Collection, Entity, ResultsAtHome,
        manga::{Chapter, Manga},
    },
    mapping::{map_result_to_chapter, map_result_to_manga, map_result_to_pages},
    query,
};

impl Mangadex {
    pub(super) fn get_manga_list(
        &self,
        mut page: i64,
        query: query::MangaList,
    ) -> Result<Vec<MangaInfo>> {
        if page < 1 {
            page = 1;
        }
        let offset = (page - 1) * 20;
        let query = query::MangaList {
            limit: 20,
            offset,
            ..query
        };

        let url = format!("{}/manga?{}", URL, query.to_query_string()?);

        let res: Collection<serde_json::Value> = self
            .client
            .fetch_json(&url)
            .with_context(|| format!("invalid MangaDex listing API response from {url}"))?;
        ensure!(
            res.result == "ok",
            "MangaDex listing API did not report success: {url}"
        );
        let Collection {
            data,
            offset: response_offset,
            total,
            ..
        } = res;
        let raw_count = data.len();
        ensure!(
            response_offset >= 0 && total >= 0,
            "invalid MangaDex listing counts from {url}"
        );
        let mut manga = Vec::new();
        for (index, record) in data.into_iter().enumerate() {
            // Decode each browsing record separately so a malformed card does not
            // discard valid siblings. Detail and chapter reads remain strict.
            match serde_json::from_value::<Manga>(record)
                .context("invalid manga record")
                .and_then(map_result_to_manga)
            {
                Ok(info) => manga.push(info),
                Err(error) => log::warn!(
                    "Skipping MangaDex listing record {} from {url}: {error:#}",
                    index + 1
                ),
            }
        }
        let rejected = raw_count - manga.len();
        if rejected > 0 {
            log::warn!("MangaDex listing from {url}: rejected {rejected} of {raw_count} records");
        }
        if manga.is_empty() && (raw_count > 0 || response_offset < total) {
            bail!(
                "MangaDex listing from {url} contains no valid manga ({raw_count} records, {rejected} rejected, offset {response_offset}, total {total})"
            );
        }
        Ok(manga)
    }
    pub(super) fn fetch_detail(&self, path: String) -> Result<MangaInfo> {
        log::debug!("{NAME}: get_manga_detail path={path}");
        let mut request_url = extension_utils::source_request_url(URL, &path)?;
        request_url.query_pairs_mut().extend_pairs([
            ("includes[]", "author"),
            ("includes[]", "artist"),
            ("includes[]", "cover_art"),
        ]);
        let url = request_url.as_str();

        let res: Entity<Manga> = self
            .client
            .fetch_json(&url)
            .with_context(|| format!("invalid MangaDex detail API response from {url}"))?;
        ensure!(
            res.result == "ok",
            "MangaDex detail API did not report success: {url}"
        );
        map_result_to_manga(res.data).with_context(|| format!("invalid MangaDex detail from {url}"))
    }

    pub(super) fn fetch_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        log::debug!("{NAME}: get_chapters path={path}");
        let mut offset = 0;
        let mut chapters = Vec::new();
        let mut expected_total = None;
        let mut seen_paths = HashSet::new();
        let mut feed_url = extension_utils::source_request_url(URL, &path)?;
        feed_url.set_path(&format!("{}/feed", feed_url.path().trim_end_matches('/')));

        loop {
            // External chapters need a separate provider extension. Setting an include
            // filter disables MangaDex's implicit published-only filter, so retain it explicitly.
            // Creation order avoids reordering releases when chapter metadata is edited.
            let mut request_url = feed_url.clone();
            request_url.query_pairs_mut().extend_pairs([
                ("limit", CHAPTER_PAGE_LIMIT.to_string()),
                ("offset", offset.to_string()),
                ("contentRating[]", "safe".into()),
                ("contentRating[]", "suggestive".into()),
                ("contentRating[]", "erotica".into()),
                ("contentRating[]", "pornographic".into()),
                ("translatedLanguage[]", "en".into()),
                ("includes[]", "scanlation_group".into()),
                ("includeExternalUrl", "0".into()),
                ("includeFuturePublishAt", "0".into()),
                ("order[createdAt]", "asc".into()),
            ]);
            let url = request_url.as_str();

            let res: Collection<Chapter> = self
                .client
                .fetch_json(&url)
                .with_context(|| format!("invalid MangaDex chapter API response from {url}"))?;
            ensure!(
                res.result == "ok",
                "MangaDex chapter API did not report success: {url}"
            );
            let Collection {
                data,
                offset: response_offset,
                total,
                limit,
                ..
            } = res;

            ensure!(
                response_offset == offset,
                "MangaDex chapter offset changed: requested {offset}, received {response_offset}: {url}"
            );
            ensure!(total >= 0, "negative MangaDex chapter total: {url}");
            ensure!(
                limit > 0 && limit <= CHAPTER_PAGE_LIMIT && data.len() <= limit as usize,
                "invalid MangaDex chapter page size: limit {limit}, records {}: {url}",
                data.len()
            );
            if let Some(expected) = expected_total {
                ensure!(
                    total == expected,
                    "MangaDex chapter total changed from {expected} to {total}; retry the refresh: {url}"
                );
            } else {
                expected_total = Some(total);
            }
            let next_offset = offset
                .checked_add(data.len() as i64)
                .context("MangaDex chapter offset overflow")?;
            ensure!(
                next_offset <= total,
                "MangaDex chapter page exceeds total {total}: {url}"
            );
            ensure!(
                !data.is_empty() || offset == total,
                "MangaDex chapter feed ended early at {offset} of {total}: {url}"
            );
            for (index, record) in data.into_iter().enumerate() {
                let chapter = map_result_to_chapter(record).with_context(|| {
                    format!("invalid MangaDex chapter record {} from {url}", index + 1)
                })?;
                // Each release ID contributes to the advertised total. Discarding a
                // duplicate would hide a missing release; chapter numbers are not IDs.
                ensure!(
                    seen_paths.insert(chapter.path.clone()),
                    "MangaDex chapter feed repeated release {}; retry the refresh: {url}",
                    chapter.path
                );
                chapters.push(chapter);
            }

            if next_offset == total {
                break;
            }
            offset = next_offset;
        }

        Ok(chapters)
    }

    pub(super) fn fetch_pages(&self, path: String) -> Result<Vec<String>> {
        log::debug!("{NAME}: get_pages path={path}");
        let mut request_url = extension_utils::source_request_url(URL, &path)?;
        let chapter_id = request_url
            .path()
            .strip_prefix("/chapter/")
            .unwrap_or(request_url.path().trim_start_matches('/'));
        let api_path = format!("/at-home/server/{chapter_id}");
        request_url.set_path(&api_path);
        let url = request_url.as_str();
        log::debug!("{NAME}: get_pages at-home url={url}");

        let res: ResultsAtHome = self
            .client_at_home
            .fetch_json(&url)
            .with_context(|| format!("invalid MangaDex At-Home API response from {url}"))?;
        map_result_to_pages(res).with_context(|| format!("invalid MangaDex pages from {url}"))
    }
}
