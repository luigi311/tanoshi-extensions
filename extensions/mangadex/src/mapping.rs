use anyhow::{Context, Result, ensure};
use extension_utils::{optional_text, unique_text};
use fancy_regex::Regex;
use std::sync::LazyLock;
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::{
    ID,
    dto::{
        ResultsAtHome,
        manga::{Chapter, ChapterRelationship, Manga, MangaAttributes, MangaRelationship, Tag},
    },
};

static BBCODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\[(\w+)[^]]*](.*?)\[/\1]"#).unwrap());

#[must_use]
fn remove_bbcode(string: String) -> String {
    let result = string
        .replace("[list]", "")
        .replace("[/list]", "")
        .replace("[*]", "")
        .replace("[hr]", "\n");
    BBCODE.replace_all(&result, "$2").to_string()
}

pub fn map_result_to_manga(data: Manga) -> Result<MangaInfo> {
    let Manga {
        kind,
        id,
        attributes,
        relationships,
    } = data;
    ensure!(kind == "manga", "expected a manga record");
    ensure!(!id.trim().is_empty(), "manga is missing its id");
    let MangaAttributes {
        mut title,
        mut description,
        status,
        tags,
    } = attributes;
    // Prefer English, romanized Japanese, then Japanese titles.
    // BTreeMap orders fallback languages by code.
    let language = ["en", "ja-ro", "ja"]
        .into_iter()
        .find(|language| {
            title
                .get(*language)
                .is_some_and(|text| !text.trim().is_empty())
        })
        .map(str::to_string)
        .or_else(|| {
            title
                .iter()
                .find(|(_, text)| !text.trim().is_empty())
                .map(|(language, _)| language.clone())
        })
        .context("manga is missing its title")?;
    let title = title
        .remove(&language)
        .context("manga is missing its title")?;
    let mut author = vec![];
    let mut file_name = String::new();
    for relationship in relationships {
        match relationship {
            MangaRelationship::Author { attributes } | MangaRelationship::Artist { attributes } => {
                if let Some(attributes) = attributes {
                    author.push(attributes.name);
                }
            }
            MangaRelationship::CoverArt { attributes } => {
                if let Some(attributes) = attributes {
                    file_name = attributes.file_name;
                }
            }
            MangaRelationship::Other => {}
        }
    }
    let genre = tags
        .into_iter()
        .filter_map(|tag| match tag {
            Tag::Tag {
                attributes: Some(mut attributes),
            } => attributes.name.remove("en"),
            _ => None,
        })
        .collect();
    Ok(MangaInfo {
        source_id: ID,
        title,
        author: unique_text(author),
        genre,
        status: status.map(|status| status.to_string()),
        description: description
            .remove("en")
            .map(remove_bbcode)
            .and_then(optional_text),
        path: format!("/manga/{id}"),
        cover_url: if file_name.trim().is_empty() {
            String::new()
        } else {
            format!("https://uploads.mangadex.org/covers/{id}/{file_name}")
        },
    })
}

pub fn map_result_to_chapter(data: Chapter) -> Result<ChapterInfo> {
    let Chapter {
        kind,
        id,
        attributes,
        relationships,
    } = data;
    ensure!(kind == "chapter", "expected a chapter record");
    ensure!(!id.trim().is_empty(), "chapter is missing its id");
    let mut scanlator = String::new();
    for relationship in relationships {
        if let ChapterRelationship::ScanlationGroup {
            attributes: Some(attributes),
        } = relationship
        {
            scanlator = attributes.name;
        }
    }
    let volume = attributes.volume;
    let number = attributes.chapter;
    let mut title = attributes.title.unwrap_or_default();
    // All three labels may be absent for a valid chapter. Keep its empty title.
    if title.is_empty() {
        if let Some(volume) = volume {
            title = format!("Volume {volume}");
        }
        if let Some(chapter) = &number {
            title = format!("{title} Chapter {chapter}");
        }
        title = title.trim().to_string();
    }
    Ok(ChapterInfo {
        source_id: ID,
        title,
        path: format!("/chapter/{id}"),
        number: number
            .and_then(|chapter| chapter.parse().ok())
            .unwrap_or_default(),
        scanlator: optional_text(scanlator),
        uploaded: attributes.publish_at.timestamp(),
    })
}

pub fn map_result_to_pages(data: ResultsAtHome) -> Result<Vec<String>> {
    ensure!(data.result == "ok", "At-Home API did not report success");
    let base_url = url::Url::parse(&data.base_url).context("invalid At-Home base URL")?;
    ensure!(
        matches!(base_url.scheme(), "http" | "https")
            && base_url.host_str().is_some()
            && base_url.query().is_none()
            && base_url.fragment().is_none(),
        "invalid At-Home image server URL"
    );
    ensure!(
        !data.chapter.hash.trim().is_empty(),
        "At-Home response is missing its chapter hash"
    );
    ensure!(
        !data.chapter.data.is_empty(),
        "At-Home response contains no pages"
    );
    data.chapter
        .data
        .iter()
        .enumerate()
        .map(|(index, file)| {
            ensure!(
                !file.trim().is_empty(),
                "At-Home page {} is missing its filename",
                index + 1
            );
            Ok(format!(
                "{}/data/{}/{}",
                data.base_url.trim_end_matches('/'),
                data.chapter.hash,
                file
            ))
        })
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;
    use serde_json::{from_value, json};

    #[test]
    fn narrow_records_accept_optional_metadata_but_require_readable_entities() {
        let minimal = json!({"type": "manga", "id": "manga-id", "attributes": {
            "title": {"en": "Title"}, "description": [],
            "tags": [{"type": "tag", "attributes": {"name": {"en": "Action"}}}]
        }, "relationships": [
            {"type": "author", "attributes": {"name": "Author"}},
            {"type": "artist", "attributes": {"name": "Artist"}},
            {"type": "future_metadata", "attributes": ["unrelated shape"]}
        ]});
        let manga = map_result_to_manga(from_value(minimal.clone()).unwrap()).unwrap();
        assert_eq!(manga.author, ["Author", "Artist"]);
        assert_eq!(manga.genre, ["Action"]);
        assert!(manga.cover_url.is_empty());
        assert_eq!(manga.description, None);
        for field in ["id", "attributes"] {
            let mut invalid = minimal.clone();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(from_value::<Manga>(invalid).is_err());
        }
        let mut wrong_kind = minimal;
        wrong_kind["type"] = json!("future_entity");
        assert!(map_result_to_manga(from_value(wrong_kind).unwrap()).is_err());

        let chapter = json!({"type": "chapter", "id": "chapter-id",
        "attributes": {"publishAt": "2020-09-20T00:00:00Z"},
        "relationships": [
            {"type": "scanlation_group", "attributes": {"name": "Group"}},
            {"type": "future_metadata", "attributes": null}
        ]});
        let info = map_result_to_chapter(from_value(chapter.clone()).unwrap()).unwrap();
        assert_eq!(info.title, "");
        assert_eq!(info.number, 0.0);
        assert_eq!(info.scanlator.as_deref(), Some("Group"));
        let mut invalid = chapter.clone();
        invalid["attributes"]
            .as_object_mut()
            .unwrap()
            .remove("publishAt");
        assert!(from_value::<Chapter>(invalid).is_err());
        let mut wrong_kind = chapter;
        wrong_kind["type"] = json!("future_entity");
        assert!(map_result_to_chapter(from_value(wrong_kind).unwrap()).is_err());
    }

    #[test]
    fn localized_title_fallback_preserves_priorities_then_sorts_language_codes() {
        for (titles, expected) in [
            (
                json!({"de": "Deutsch", "ja": "日本語", "ja-ro": "Romaji", "en": "English"}),
                "English",
            ),
            (
                json!({"en": " ", "ja": "日本語", "ja-ro": "Romaji", "de": "Deutsch"}),
                "Romaji",
            ),
            (json!({"de": "Deutsch", "ja": "日本語"}), "日本語"),
            (
                json!({"fr": "Français", "de": "Deutsch", "ar": " "}),
                "Deutsch",
            ),
        ] {
            let record = json!({"type": "manga", "id": "id", "attributes": {"title": titles}});
            assert_eq!(
                map_result_to_manga(from_value(record).unwrap())
                    .unwrap()
                    .title,
                expected
            );
        }
        for titles in [json!({}), json!([]), json!({"en": " "})] {
            let record = json!({"type": "manga", "id": "id", "attributes": {"title": titles}});
            assert!(map_result_to_manga(from_value(record).unwrap()).is_err());
        }
    }
}
