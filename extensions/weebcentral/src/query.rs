use crate::URL;
use anyhow::{Context, Result};

pub(super) enum Sort {
    Popularity,
    LatestUpdates,
}

pub(super) struct ListingQuery<'a> {
    pub(super) page: i64,
    pub(super) sort: Sort,
    // None omits both text and author, as the latest feed currently does.
    pub(super) text: Option<&'a str>,
}

impl ListingQuery<'_> {
    pub(super) fn url(self) -> Result<String> {
        let offset = (self.page.max(1) - 1)
            .checked_mul(32)
            .context("WeebCentral listing page offset is too large")?;
        let mut url = extension_utils::source_request_url(URL, "/search/data")?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", "32");
            if let Some(text) = self.text {
                query.append_pair("author", "").append_pair("text", text);
            }
            let sort = match self.sort {
                Sort::Popularity => "Popularity",
                Sort::LatestUpdates => "Latest Updates",
            };
            query.extend_pairs([
                ("sort", sort),
                ("order", "Descending"),
                ("official", "Any"),
                ("anime", "Any"),
                ("adult", "Any"),
                ("display_mode", "Full Display"),
            ]);
            query.append_pair("offset", &offset.to_string());
        }
        Ok(url.into())
    }
}
