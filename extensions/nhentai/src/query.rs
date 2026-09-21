use crate::{NHentai, URL};
use anyhow::{Result, anyhow};
use lazy_static::lazy_static;
use tanoshi_lib::prelude::{Input, InputType};
use urlencoding::encode;

lazy_static! {
    pub(super) static ref TAG_FILTER: Input = Input::Text {
        name: "Tag".to_string(),
        state: None
    };
    pub(super) static ref CHARACTERS_FILTER: Input = Input::Text {
        name: "Characters".to_string(),
        state: None
    };
    pub(super) static ref ARTISTS_FILTER: Input = Input::Text {
        name: "Artists".to_string(),
        state: None
    };
    pub(super) static ref GROUPS_FILTER: Input = Input::Text {
        name: "Groups".to_string(),
        state: None
    };
    pub(super) static ref CATEGORIES_FILTER: Input = Input::Text {
        name: "Categories".to_string(),
        state: None
    };
    pub(super) static ref PARODIES_FILTER: Input = Input::Text {
        name: "Parodies".to_string(),
        state: None
    };
    pub(super) static ref SORT_FILTER: Input = Input::Select {
        name: "Sort".to_string(),
        values: vec![
            InputType::String("Popular".to_string()),
            InputType::String("Popular Week".to_string()),
            InputType::String("Popular Today".to_string()),
            InputType::String("Recent".to_string()),
        ],
        state: None
    };
    pub(super) static ref FILTER_LIST: Vec<Input> = vec![
        TAG_FILTER.clone(),
        CHARACTERS_FILTER.clone(),
        CATEGORIES_FILTER.clone(),
        PARODIES_FILTER.clone(),
        ARTISTS_FILTER.clone(),
        GROUPS_FILTER.clone(),
        SORT_FILTER.clone()
    ];
    pub(super) static ref LANGUAGE_SELECT: Input = Input::Select {
        name: "Language".to_string(),
        values: vec![
            InputType::String("Any".to_string()),
            InputType::String("English".to_string()),
            InputType::String("Japanese".to_string()),
            InputType::String("Chinese".to_string()),
        ],
        state: None
    };
    pub(super) static ref BLACKLIST_TAG: Input = Input::Text {
        name: "Blacklist Tag".to_string(),
        state: None
    };
    pub(super) static ref PREFERENCES: Vec<Input> =
        vec![LANGUAGE_SELECT.clone(), BLACKLIST_TAG.clone()];
}

fn nh_field_key(ui_label: &str) -> Option<&'static str> {
    match ui_label {
        "Tag" => Some("tag"),
        "Characters" => Some("character"),
        "Artists" => Some("artist"),
        "Groups" => Some("group"),
        "Categories" => Some("category"),
        "Parodies" => Some("parody"),
        _ => None,
    }
}

pub(super) struct SearchQuery {
    pub(super) text: String,
    sort: Option<String>,
}

fn norm_value(v: &str) -> String {
    // NH prefers underscores for multi-word tokens
    v.trim().replace(' ', "_")
}

impl NHentai {
    pub(super) fn search_url(
        &self,
        page: i64,
        query: Option<String>,
        filters: Option<Vec<Input>>,
    ) -> Result<String> {
        let query = query.filter(|text| !text.trim().is_empty());
        let filters = filters.filter(|filters| !filters.is_empty());
        if query.is_none() && filters.is_none() {
            return Err(anyhow!("query and filters cannot be both empty"));
        }
        let text_only = filters.is_none();
        let mut request = self.query_parts(query.as_deref(), filters)?;
        // Text-only searches sort by popularity; filtered searches use their selected sort.
        if text_only {
            request.sort = Some("popular".to_string());
        }
        let q = encode(&request.text);
        Ok(match request.sort {
            Some(sort) => format!("{URL}/search/?q={q}&sort={sort}&page={page}"),
            None => format!("{URL}/search/?q={q}&page={page}"),
        })
    }

    pub(super) fn query_parts(
        &self,
        text: Option<&str>,
        filters: Option<Vec<Input>>,
    ) -> Result<SearchQuery> {
        let mut query: Vec<String> = text
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string)
            .into_iter()
            .collect();
        let mut sort: Option<String> = None;

        // preferences: language + global blacklist
        for pref in self.preferences.iter() {
            if LANGUAGE_SELECT.eq(pref)
                && let Input::Select { state, values, .. } = pref
                && let Some(InputType::String(lang)) = state.and_then(|i| values.get(i as usize))
                && lang != "Any"
            {
                query.push(format!("language:{}", lang.to_lowercase()));
            } else if BLACKLIST_TAG.eq(pref)
                && let Input::Text {
                    state: Some(state), ..
                } = pref
            {
                for tag in state.split(',') {
                    let t = norm_value(tag);
                    if !t.is_empty() {
                        query.push(format!("-tag:{t}"));
                    }
                }
            }
        }

        // filters
        if let Some(filters) = filters {
            for filter in filters {
                let Some(canonical) = FILTER_LIST
                    .iter()
                    .find(|known| known.name() == filter.name())
                else {
                    continue;
                };
                if !canonical.eq(&filter) {
                    return Err(anyhow!("invalid {}: unexpected input type", filter.name()));
                }
                match filter {
                    Input::Text {
                        name,
                        state: Some(state),
                        ..
                    } if name == TAG_FILTER.name() => {
                        let Some(key) = nh_field_key(&name) else {
                            continue;
                        };
                        for raw in state.split(',') {
                            let raw = raw.trim();
                            if raw.is_empty() {
                                continue;
                            }
                            let neg = raw.starts_with('-');
                            let term = norm_value(raw.trim_start_matches('-'));
                            if term.is_empty() {
                                continue;
                            }
                            if neg {
                                query.push(format!("-{key}:{term}"));
                            } else {
                                query.push(format!("{key}:{term}"));
                            }
                        }
                    }
                    Input::Text {
                        name,
                        state: Some(state),
                        ..
                    } => {
                        let Some(key) = nh_field_key(&name) else {
                            continue;
                        };
                        let term = norm_value(&state);
                        if !term.is_empty() {
                            query.push(format!("{key}:{term}"));
                        }
                    }
                    Input::Select {
                        name,
                        values,
                        state,
                        ..
                    } if name == SORT_FILTER.name() => {
                        let index = state.unwrap_or(0);
                        let selected = usize::try_from(index)
                            .ok()
                            .and_then(|index| values.get(index));
                        let Some(InputType::String(value)) = selected else {
                            return Err(anyhow!(
                                "invalid Sort: selection index {index} is not a string choice"
                            ));
                        };
                        if !matches!(
                            value.as_str(),
                            "Popular" | "Popular Week" | "Popular Today" | "Recent"
                        ) {
                            return Err(anyhow!("invalid Sort: unknown choice"));
                        }
                        sort = Some(value.replace(' ', "-").to_lowercase());
                    }
                    _ => {}
                }
            }
        }

        let q = if query.is_empty() {
            r#""""#.to_string()
        } else {
            query.join(" ")
        };
        Ok(SearchQuery { text: q, sort })
    }
}
