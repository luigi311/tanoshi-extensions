use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::BTreeMap, fmt::Display, str::FromStr};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ongoing,
    Completed,
    Hiatus,
    Cancelled,
}

impl Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Ongoing => write!(f, "ongoing"),
            Status::Completed => write!(f, "completed"),
            Status::Hiatus => write!(f, "hiatus"),
            Status::Cancelled => write!(f, "canceled"),
        }
    }
}

impl FromStr for Status {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ongoing" => Ok(Status::Ongoing),
            "completed" => Ok(Status::Completed),
            "hiatus" => Ok(Status::Hiatus),
            "canceled" => Ok(Status::Cancelled),
            _ => Err(anyhow::anyhow!("no such status")),
        }
    }
}

// Some MangaDex localized-text fields encode an empty object as [].
// Nonempty arrays are not a supported text map.
type LocalizedText = BTreeMap<String, String>;
fn localized_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<LocalizedText, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum TextOrEmpty {
        Text(LocalizedText),
        Empty([(); 0]),
    }
    Ok(match TextOrEmpty::deserialize(deserializer)? {
        TextOrEmpty::Text(text) => text,
        TextOrEmpty::Empty(_) => LocalizedText::new(),
    })
}

#[derive(Debug, Deserialize)]
pub struct Manga {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    pub attributes: MangaAttributes,
    #[serde(default)]
    pub relationships: Vec<MangaRelationship>,
}

#[derive(Debug, Deserialize)]
pub struct MangaAttributes {
    #[serde(deserialize_with = "localized_text")]
    pub title: LocalizedText,
    #[serde(default, deserialize_with = "localized_text")]
    pub description: LocalizedText,
    pub status: Option<Status>,
    #[serde(default)]
    pub tags: Vec<Tag>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MangaRelationship {
    Author {
        attributes: Option<NameAttributes>,
    },
    Artist {
        attributes: Option<NameAttributes>,
    },
    CoverArt {
        attributes: Option<CoverAttributes>,
    },
    // Ignore metadata relationships this endpoint's mapper does not consume.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct NameAttributes {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverAttributes {
    pub file_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tag {
    Tag {
        attributes: Option<TagAttributes>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct TagAttributes {
    #[serde(deserialize_with = "localized_text")]
    pub name: LocalizedText,
}

#[derive(Debug, Deserialize)]
pub struct Chapter {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    pub attributes: ChapterAttributes,
    #[serde(default)]
    pub relationships: Vec<ChapterRelationship>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterAttributes {
    pub title: Option<String>,
    pub volume: Option<String>,
    pub chapter: Option<String>,
    pub publish_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChapterRelationship {
    ScanlationGroup {
        attributes: Option<NameAttributes>,
    },
    #[serde(other)]
    Other,
}
