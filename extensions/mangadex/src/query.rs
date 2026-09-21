use anyhow::{Context, bail};
use tanoshi_lib::prelude::{Input, InputType, TriState};

use crate::filter::*;

use crate::dto::manga::Status;
use serde::Serialize;
use std::fmt::Display;
use std::str::FromStr;

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    Desc,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListOrder {
    pub latest_uploaded_chapter: Option<Order>,
    pub followed_count: Option<Order>,
}

#[derive(Debug, Serialize)]
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
    pub content_rating: Vec<Rating>,
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
            content_rating: Default::default(),
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
        // Text-only searches include all four content ratings. Filtered searches
        // use their selections, including the UI's three-rating default.
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
    if let (Input::Group { state, .. }, Input::Group { state: known, .. }) = (input, canonical) {
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
                                    .and_then(|name| tag_id(&name).map(str::to_string))
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
                                    .and_then(|name| tag_id(&name).map(str::to_string))
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
                if content_rating.is_empty() {
                    bail!("Select at least one content rating");
                }
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

#[cfg(test)]
mod test {
    use tanoshi_lib::prelude::Input;

    use super::{ListOrder, MangaList, Order};

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
        let mut unchecked = crate::filter::CONTENT_RATING_FILTER.clone();
        if let Input::Group { state, .. } = &mut unchecked {
            for input in state {
                if let Input::Checkbox { state, .. } = input {
                    *state = Some(false);
                }
            }
        }
        let error = MangaList::search(Some(text.to_string()), Some(vec![unchecked])).unwrap_err();
        assert_eq!(error.to_string(), "Select at least one content rating");
        let omitted = MangaList::search(
            Some(text.to_string()),
            Some(vec![Input::Text {
                name: "Title".into(),
                state: None,
            }]),
        )
        .unwrap();
        assert!(!omitted.to_query_string().unwrap().contains("contentRating"));
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
