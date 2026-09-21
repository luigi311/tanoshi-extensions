//! Shared Madara support for Manhwa18cc and the retained Tritinia/HariManga candidates.
//! See the crate README for protocol ownership and restoration limitations.

mod api;
mod parse;

pub use api::{
    DetailClient, fetch_ajax_chapters, fetch_ajax_latest, fetch_ajax_popular, fetch_html_chapters,
    fetch_pages, get_manga_detail, search_ajax, search_html,
};
pub use parse::parse_manga_list;

#[cfg(test)]
mod test {
    use super::*;
    use scraper::Selector;

    struct StaticClient(&'static str);

    impl DetailClient for StaticClient {
        fn fetch_document(&self, url: &str) -> anyhow::Result<networking::FetchedDocument> {
            Ok(networking::FetchedDocument {
                final_url: url.to_string(),
                body: self.0.to_string(),
            })
        }
    }

    #[test]
    fn get_manga_detail_missing_title_returns_error() {
        let result = get_manga_detail(
            "https://example.test",
            "/missing",
            1,
            &StaticClient("<html><body>No title</body></html>"),
        );

        let error = result.expect_err("missing title should return an error");
        assert_eq!(
            error.to_string(),
            "no title found at https://example.test/missing"
        );
    }

    #[test]
    fn get_manga_detail_collects_fully_wrapped_title_text() {
        // Title entirely inside a child element: no trailing text node, so
        // the collected-text fallback must kick in.
        let result = get_manga_detail(
            "https://example.test",
            "/wrapped",
            1,
            &StaticClient(r#"<div class="post-title"><h1><span>Wrapped title</span></h1></div>"#),
        )
        .expect("wrapped title should parse");

        assert_eq!(result.title, "Wrapped title");
    }

    #[test]
    fn get_manga_detail_skips_badge_span_before_title() {
        // Regression: manhwa18.cc marks adult series with a badge span inside
        // the title element; it must not be glued onto the title.
        let result = get_manga_detail(
            "https://example.test",
            "/badged",
            1,
            &StaticClient(
                r#"<div class="post-title"><h1><span>18+</span> Private Tutoring</h1></div>"#,
            ),
        )
        .expect("badged title should parse");

        assert_eq!(result.title, "Private Tutoring");
    }

    #[test]
    fn parse_manga_list_empty_markup_returns_error() {
        let selector = Selector::parse(".card").unwrap();
        let result = parse_manga_list(
            "https://example.test",
            1,
            &networking::FetchedDocument {
                final_url: "https://example.test/".into(),
                body: "<html><body>unexpected response</body></html>".into(),
            },
            &selector,
        );

        let error = result.expect_err("empty parsed list should return an error");
        assert_eq!(
            error.to_string(),
            "parsed 0 items from https://example.test — markup change?"
        );
    }
}
