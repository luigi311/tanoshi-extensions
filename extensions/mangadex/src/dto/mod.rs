use serde::Deserialize;

pub mod manga;

#[derive(Debug, Deserialize)]
pub struct Entity<T> {
    pub result: String,
    pub data: T,
}

#[derive(Debug, Deserialize)]
pub struct Collection<T> {
    pub result: String,
    pub data: Vec<T>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultsAtHome {
    pub result: String,
    pub base_url: String,
    pub chapter: AtHomeChapter,
}

#[derive(Debug, Deserialize)]
pub struct AtHomeChapter {
    pub hash: String,
    pub data: Vec<String>,
}
