use std::{collections::HashMap, fmt::Display, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::marker::PhantomData;

use super::Relationship;
use serde::de::{self, MapAccess, Visitor};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TagMode {
    And,
    Or,
}

impl FromStr for TagMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "AND" => Ok(TagMode::And),
            "OR" => Ok(TagMode::Or),
            _ => Err(anyhow::anyhow!("no such status")),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Demographic {
    Shounen,
    Shoujo,
    Josei,
    Seinen,
    None,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Rating {
    Safe,
    Suggestive,
    Erotica,
    Pornographic,
}

impl Display for Rating {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rating::Safe => write!(f, "safe"),
            Rating::Suggestive => write!(f, "suggestive"),
            Rating::Erotica => write!(f, "erotica"),
            Rating::Pornographic => write!(f, "pornographic"),
        }
    }
}

impl FromStr for Rating {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "safe" => Ok(Rating::Safe),
            "suggestive" => Ok(Rating::Suggestive),
            "erotica" => Ok(Rating::Erotica),
            "pornographic" => Ok(Rating::Pornographic),
            _ => Err(anyhow::anyhow!("no such rating")),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListOrder {
    pub created_at: Option<Order>,
    pub updated_at: Option<Order>,
    pub latest_uploaded_chapter: Option<Order>,
    pub followed_count: Option<Order>,
}

pub type Map = HashMap<String, String>;

fn sequence_or_map<'de, D>(deserializer: D) -> Result<Map, D::Error>
where
    D: Deserializer<'de>,
{
    // This is a Visitor that forwards string types to T's `FromStr` impl and
    // forwards map types to T's `Deserialize` impl. The `PhantomData` is to
    // keep the compiler from complaining about T being an unused generic type
    // parameter. We need T in order to know the Value type for the Visitor
    // impl.
    struct SequenceOrMap<Map>(PhantomData<fn() -> Map>);

    impl<'de> Visitor<'de> for SequenceOrMap<Map> {
        type Value = Map;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("sequence or map")
        }

        fn visit_seq<A>(self, _seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            Ok(HashMap::new())
        }

        fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            // `MapAccessDeserializer` is a wrapper that turns a `MapAccess`
            // into a `Deserializer`, allowing it to be used as the input to T's
            // `Deserialize` implementation. T then deserializes itself using
            // the entries from the map visitor.
            Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_any(SequenceOrMap(PhantomData))
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TagAttributes {
    #[serde(deserialize_with = "sequence_or_map")]
    pub name: Map,
    #[serde(deserialize_with = "sequence_or_map")]
    pub description: Map,
    pub group: String,
    pub version: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MangaAttributes {
    #[serde(deserialize_with = "sequence_or_map")]
    pub title: Map,
    pub alt_titles: Vec<Map>,
    #[serde(deserialize_with = "sequence_or_map")]
    pub description: Map,
    #[serde(default = "bool::default")]
    pub is_locked: bool,
    // pub links: Option<Map>,
    pub original_language: String,
    pub last_volume: Option<String>,
    pub last_chapter: Option<String>,
    pub publication_demographic: Option<Demographic>,
    pub status: Option<Status>,
    pub year: Option<i64>,
    pub content_rating: Rating,
    pub tags: Vec<Relationship>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorAttributes {
    pub name: String,
    pub image_url: Option<String>,
    #[serde(deserialize_with = "sequence_or_map")]
    pub biography: Map,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverAttributes {
    pub volume: Option<String>,
    pub file_name: String,
    pub description: String,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanlationGroupAttributes {
    pub name: String,
    pub website: Option<String>,
    pub irc_server: Option<String>,
    pub irc_channel: Option<String>,
    pub discord: Option<String>,
    pub contact_email: Option<String>,
    pub description: Option<String>,
    pub locked: bool,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterAttributes {
    pub title: Option<String>,
    pub volume: Option<String>,
    pub chapter: Option<String>,
    pub translated_language: String,
    pub external_url: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub publish_at: DateTime<Utc>,
    pub pages: i64,
}

pub mod request {
    use anyhow::{Context, bail};
    use tanoshi_lib::prelude::{Input, InputType, TriState};

    use crate::filter::*;

    use super::*;

    #[derive(Debug, Clone, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct MangaList {
        pub limit: i64,
        pub offset: i64,
        pub title: Option<String>,
        pub authors: Vec<String>,
        pub artists: Vec<String>,
        pub year: Option<i64>,
        pub included_tags: Vec<String>,
        pub included_tags_mode: Option<TagMode>,
        pub excluded_tags: Vec<String>,
        pub excluded_tags_mode: Option<TagMode>,
        pub status: Vec<Status>,
        pub original_language: Vec<String>,
        pub publication_demographic: Vec<Demographic>,
        pub ids: Vec<String>,
        pub content_rating: Vec<Rating>,
        pub created_at_since: Option<DateTime<Utc>>,
        pub updated_at_since: Option<DateTime<Utc>>,
        pub order: Option<ListOrder>,
        pub includes: Vec<String>,
    }

    impl Default for MangaList {
        fn default() -> Self {
            Self {
                includes: vec![
                    "cover_art".to_string(),
                    "author".to_string(),
                    "artist".to_string(),
                    "scanlation_group".to_string(),
                ],
                limit: Default::default(),
                offset: Default::default(),
                title: Default::default(),
                authors: Default::default(),
                artists: Default::default(),
                year: Default::default(),
                included_tags: Default::default(),
                included_tags_mode: Default::default(),
                excluded_tags: Default::default(),
                excluded_tags_mode: Default::default(),
                status: Default::default(),
                original_language: Default::default(),
                publication_demographic: Default::default(),
                ids: Default::default(),
                content_rating: Default::default(),
                created_at_since: Default::default(),
                updated_at_since: Default::default(),
                order: Default::default(),
            }
        }
    }

    impl MangaList {
        pub fn search(query: Option<String>, filters: Option<Vec<Input>>) -> anyhow::Result<Self> {
            let query = query.filter(|text| !text.trim().is_empty());
            if let Some(filters) = filters.filter(|filters| !filters.is_empty()) {
                let mut request = Self::try_from(filters)?;
                if query.is_some() {
                    request.title = query;
                }
                return Ok(request);
            }
            let Some(query) = query else {
                bail!("query and filters cannot be both empty");
            };
            // Preserve the existing text-only rating policy. Filtered searches
            // retain their own selections, including the UI's three-rating default.
            Ok(Self {
                title: Some(query),
                content_rating: vec![
                    Rating::Safe,
                    Rating::Suggestive,
                    Rating::Erotica,
                    Rating::Pornographic,
                ],
                ..Default::default()
            })
        }

        pub fn to_query_string(&self) -> anyhow::Result<String> {
            Ok(serde_qs::to_string(self)?)
        }
    }

    fn validate_filter_shape(input: &Input, canonical: &Input) -> anyhow::Result<()> {
        if !input.eq(canonical) {
            bail!("invalid {}: unexpected input type", canonical.name());
        }
        if let (Input::Group { state, .. }, Input::Group { state: known, .. }) = (input, canonical)
        {
            for child in state {
                if let Some(expected) = known.iter().find(|item| item.name() == child.name()) {
                    validate_filter_shape(child, expected)?;
                }
            }
        }
        Ok(())
    }

    fn selected_tag_mode(
        values: &[InputType],
        state: Option<i64>,
        name: &str,
    ) -> anyhow::Result<Option<TagMode>> {
        let Some(index) = state else {
            return Ok(None);
        };
        let selected = usize::try_from(index)
            .ok()
            .and_then(|index| values.get(index));
        let Some(InputType::String(mode)) = selected else {
            bail!("invalid {name}: selection index {index} is not a string choice");
        };
        Ok(Some(TagMode::from_str(mode).with_context(|| {
            format!("invalid {name}: expected AND or OR")
        })?))
    }

    impl TryFrom<Vec<Input>> for MangaList {
        type Error = anyhow::Error;

        fn try_from(filters: Vec<Input>) -> anyhow::Result<Self> {
            let mut included_tags = vec![];
            let mut included_tags_mode = None;
            let mut excluded_tags = vec![];
            let mut excluded_tags_mode = None;
            let mut status = vec![];
            let mut content_rating = vec![];
            let mut title = None;
            let mut year = None;
            let mut artists = vec![];
            let mut authors = vec![];

            for filter in filters {
                let Some(canonical) = FILTER_LIST
                    .iter()
                    .find(|known| known.name() == filter.name())
                else {
                    continue;
                };
                validate_filter_shape(&filter, canonical)?;
                if TITLE_FILTER.eq(&filter) {
                    if let Input::Text { state, .. } = filter {
                        title = state.filter(|text| !text.trim().is_empty());
                    }
                } else if AUTHOR_FILTER.eq(&filter) {
                    if let Input::Text { state, .. } = filter {
                        authors = state
                            .map(|s| {
                                s.split(',')
                                    .map(str::trim)
                                    .filter(|id| !id.is_empty())
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default();
                    }
                } else if ARTIST_FILTER.eq(&filter) {
                    if let Input::Text { state, .. } = filter {
                        artists = state
                            .map(|s| {
                                s.split(',')
                                    .map(str::trim)
                                    .filter(|id| !id.is_empty())
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default();
                    }
                } else if YEAR_FILTER.eq(&filter) {
                    if let Input::Text { state, .. } = filter {
                        year = state
                            .filter(|year| !year.trim().is_empty())
                            .map(|year| {
                                year.trim()
                                    .parse()
                                    .context("invalid Year: expected an integer")
                            })
                            .transpose()?;
                    }
                } else if TAG_FILTERS.eq(&filter) {
                    if let Input::Group { state, .. } = filter {
                        included_tags = state
                            .iter()
                            .filter_map(|s| {
                                if let Input::State { name, selected } = s {
                                    (selected.unwrap_or_default() == TriState::Included)
                                        .then(|| name.clone())
                                        .and_then(|name| {
                                            TAG_ID_MAP.get(&name).map(|id| id.to_string())
                                        })
                                } else {
                                    None
                                }
                            })
                            .collect();

                        excluded_tags = state
                            .iter()
                            .filter_map(|s| {
                                if let Input::State { name, selected } = s {
                                    (selected.unwrap_or_default() == TriState::Excluded)
                                        .then(|| name.clone())
                                        .and_then(|name| {
                                            TAG_ID_MAP.get(&name).map(|id| id.to_string())
                                        })
                                } else {
                                    None
                                }
                            })
                            .collect();
                    }
                } else if INCLUDED_TAGS_MODE.eq(&filter) {
                    if let Input::Select { values, state, .. } = filter {
                        included_tags_mode =
                            selected_tag_mode(&values, state, &INCLUDED_TAGS_MODE.name())?;
                    }
                } else if EXCLUDED_TAGS_MODE.eq(&filter) {
                    if let Input::Select { values, state, .. } = filter {
                        excluded_tags_mode =
                            selected_tag_mode(&values, state, &EXCLUDED_TAGS_MODE.name())?;
                    }
                } else if STATUS_FILTER.eq(&filter) {
                    if let Input::Group { state, .. } = filter {
                        status = state
                            .iter()
                            .filter_map(|input| {
                                if let Input::Checkbox { name, state } = input {
                                    if state.unwrap_or(false) {
                                        Status::from_str(name).ok()
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            })
                            .collect();
                    }
                } else if CONTENT_RATING_FILTER.eq(&filter)
                    && let Input::Group { state, .. } = filter
                {
                    content_rating = state
                        .iter()
                        .filter_map(|input| {
                            if let Input::Checkbox { name, state } = input {
                                if state.unwrap_or(false) {
                                    Rating::from_str(name).ok()
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .collect();
                }
            }

            Ok(Self {
                title,
                authors,
                artists,
                year,
                included_tags,
                included_tags_mode,
                excluded_tags,
                excluded_tags_mode,
                status,
                content_rating,
                ..Default::default()
            })
        }
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    pub struct Manga {
        pub includes: Vec<String>,
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    pub struct MangaFeed {
        pub limit: i64,
        pub offset: i64,
        pub translated_language: Vec<String>,
        pub created_at_since: Option<DateTime<Utc>>,
        pub updated_at_since: Option<DateTime<Utc>>,
        pub published_at_since: Option<DateTime<Utc>>,
        pub order: Option<ListOrder>,
        pub includes: Vec<String>,
    }
}

#[cfg(test)]
mod test {
    use tanoshi_lib::prelude::Input;

    use super::{ListOrder, Order, request::MangaList};

    #[test]
    fn test_input_to_manga_list_request() {
        let input = vec![
            Input::Group {
                name: "Status".to_string(),
                state: vec![
                    Input::Checkbox {
                        name: "ongoing".to_string(),
                        state: Some(true),
                    },
                    Input::Checkbox {
                        name: "completed".to_string(),
                        state: Some(true),
                    },
                    Input::Checkbox {
                        name: "hiatus".to_string(),
                        state: Some(true),
                    },
                    Input::Checkbox {
                        name: "canceled".to_string(),
                        state: Some(true),
                    },
                ],
            },
            Input::Group {
                name: "Content Rating".to_string(),
                state: vec![
                    Input::Checkbox {
                        name: "safe".to_string(),
                        state: Some(true),
                    },
                    Input::Checkbox {
                        name: "suggestive".to_string(),
                        state: Some(true),
                    },
                    Input::Checkbox {
                        name: "erotica".to_string(),
                        state: Some(true),
                    },
                    Input::Checkbox {
                        name: "pornographic".to_string(),
                        state: Some(true),
                    },
                ],
            },
        ];

        let manga_list = MangaList::try_from(input).unwrap();
        let query = manga_list.to_query_string().unwrap();
        assert_eq!(
            "limit=0&offset=0&status[0]=ongoing&status[1]=completed&status[2]=hiatus&status[3]=cancelled&contentRating[0]=safe&contentRating[1]=suggestive&contentRating[2]=erotica&contentRating[3]=pornographic&includes[0]=cover_art&includes[1]=author&includes[2]=artist&includes[3]=scanlation_group",
            query,
            "expected got {query}"
        );
        use tanoshi_lib::prelude::InputType;
        for filter in [
            Input::Text {
                name: "Year".into(),
                state: Some("bad".into()),
            },
            Input::Checkbox {
                name: "Title".into(),
                state: Some(true),
            },
            Input::Select {
                name: "Included Tags Mode".into(),
                values: vec![InputType::String("AND".into())],
                state: Some(-1),
            },
            Input::Select {
                name: "Excluded Tags Mode".into(),
                values: vec![],
                state: Some(0),
            },
            Input::Select {
                name: "Included Tags Mode".into(),
                values: vec![InputType::String("XOR".into())],
                state: Some(0),
            },
        ] {
            let name = filter.name();
            assert!(
                format!("{:#}", MangaList::try_from(vec![filter]).unwrap_err()).contains(&name)
            );
        }
        let request = MangaList::try_from(vec![
            Input::Text {
                name: "Author".into(),
                state: Some(" id-a, ,id-b ,".into()),
            },
            Input::Text {
                name: "Artist".into(),
                state: Some(" , id-c, ".into()),
            },
            Input::Text {
                name: "Year".into(),
                state: Some(" 2020 ".into()),
            },
            Input::Select {
                name: "Future Field".into(),
                values: vec![],
                state: Some(-1),
            },
        ])
        .unwrap();
        assert_eq!(request.authors, ["id-a", "id-b"]);
        assert_eq!(request.artists, ["id-c"]);
        assert_eq!(request.year, Some(2020));
    }

    #[test]
    fn test_search_composes_text_and_filters() {
        let text = "A&B + 日本語";
        let plain = MangaList::search(Some(text.to_string()), None).unwrap();
        let empty = MangaList::search(Some(text.to_string()), Some(vec![])).unwrap();
        assert_eq!(
            plain.to_query_string().unwrap(),
            empty.to_query_string().unwrap()
        );
        assert_eq!(plain.title.as_deref(), Some(text));
        assert_eq!(plain.content_rating.len(), 4);
        let encoded = plain.to_query_string().unwrap();
        assert!(encoded.contains("A%26B"));
        assert!(encoded.contains("%2B"));
        assert!(encoded.contains("%E6%97%A5%E6%9C%AC%E8%AA%9E"));

        let filters = vec![
            Input::Text {
                name: "Title".to_string(),
                state: Some("fallback".to_string()),
            },
            crate::filter::CONTENT_RATING_FILTER.clone(),
        ];
        for query in [None, Some("  ".to_string()), Some(text.to_string())] {
            let expected = if query.as_deref() == Some(text) {
                text
            } else {
                "fallback"
            };
            let composed = MangaList::search(query, Some(filters.clone())).unwrap();
            assert_eq!(composed.title.as_deref(), Some(expected));
            assert_eq!(composed.content_rating.len(), 3);
            assert!(!composed.to_query_string().unwrap().contains("pornographic"));
        }
        let explicit = Input::Group {
            name: "Content Rating".to_string(),
            state: vec![Input::Checkbox {
                name: "safe".to_string(),
                state: Some(true),
            }],
        };
        let composed = MangaList::search(Some(text.to_string()), Some(vec![explicit])).unwrap();
        assert_eq!(composed.content_rating.len(), 1);
        for filters in [None, Some(vec![])] {
            assert!(MangaList::search(None, filters.clone()).is_err());
            assert!(MangaList::search(Some("  ".to_string()), filters).is_err());
        }
    }

    #[test]
    fn test_latest_uploaded_chapter_order_query() {
        let query = MangaList {
            order: Some(ListOrder {
                latest_uploaded_chapter: Some(Order::Desc),
                ..Default::default()
            }),
            ..Default::default()
        }
        .to_query_string()
        .unwrap();

        assert!(query.contains("order[latestUploadedChapter]=desc"));
    }
}
