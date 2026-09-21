use super::api::{GalleryApiPage, GalleryApiResponse};
use super::parse::{build_gallery_page_urls, parse_gallery, parse_uploaded_timestamp};
use super::query::{PARODIES_FILTER, SORT_FILTER, TAG_FILTER};
use super::*;
use extension_utils::element_text as trimmed_element_text;
use networking::{FetchedDocument, parse_browser_json};
use scraper::{Html, Selector};
use std::time::{Duration, Instant};
use tanoshi_lib::prelude::InputType;

fn create_test_instance() -> NHentai {
    let preferences: Vec<Input> = vec![
        Input::Text {
            name: "Blacklist Tag".to_string(),
            state: Some("posession".to_string()),
        },
        Input::Select {
            name: "Language".to_string(),
            values: vec![
                InputType::String("Any".to_string()),
                InputType::String("English".to_string()),
                InputType::String("Japanese".to_string()),
                InputType::String("Chinese".to_string()),
            ],
            state: Some(1),
        },
    ];

    let mut nhentai: NHentai = NHentai::default();

    nhentai.set_preferences(preferences).unwrap();

    nhentai
}

#[test]
fn search_combines_text_filters_and_preferences() {
    let source = create_test_instance();
    let text = "A&B + 日本語";
    let text_filter = |name: &str, state: &str| Input::Text {
        name: name.into(),
        state: Some(state.into()),
    };
    let query_value = |url: &str| {
        let value = url
            .split('?')
            .nth(1)
            .unwrap()
            .split('&')
            .find_map(|part| part.strip_prefix("q="))
            .unwrap();
        urlencoding::decode(value).unwrap().into_owned()
    };
    let plain = source.search_url(1, Some(text.into()), None).unwrap();
    assert!(query_value(&plain).contains(text));
    assert!(query_value(&plain).contains("language:english"));
    assert!(query_value(&plain).contains("-tag:posession"));
    assert!(plain.contains("sort=popular&"));
    assert_eq!(
        plain,
        source
            .search_url(1, Some(text.into()), Some(vec![]))
            .unwrap()
    );
    let filters = vec![
        text_filter("Tag", "romance, -big breasts, , -"),
        text_filter("Future Field", "ignored"),
        Input::Select {
            name: "Sort".into(),
            values: vec![InputType::String("Popular Week".into())],
            state: Some(0),
        },
    ];
    for query in [None, Some(" ".into()), Some(text.into())] {
        let combined = source
            .search_url(2, query.clone(), Some(filters.clone()))
            .unwrap();
        let decoded = query_value(&combined);
        assert_eq!(decoded.contains(text), query.as_deref() == Some(text));
        for term in [
            "language:english",
            "-tag:posession",
            "tag:romance",
            "-tag:big_breasts",
        ] {
            assert!(decoded.contains(term), "missing {term}: {decoded}");
        }
        assert!(!decoded.contains("ignored"));
        assert!(!decoded.split_whitespace().any(|term| term == "-tag:"));
        assert!(combined.ends_with("sort=popular-week&page=2"));
    }
    for state in [-1, 4] {
        let filters = vec![Input::Select {
            name: "Sort".into(),
            values: vec![InputType::String("Popular".into())],
            state: Some(state),
        }];
        assert!(
            source
                .search_url(1, Some(text.into()), Some(filters))
                .unwrap_err()
                .to_string()
                .contains("Sort")
        );
    }
    for filters in [None, Some(vec![])] {
        assert!(source.search_url(1, None, filters.clone()).is_err());
        assert!(source.search_url(1, Some(" ".into()), filters).is_err());
    }
}

#[test]
fn gallery_cache_keeps_validated_metadata_with_existing_expiry_and_lru_policy() {
    let now = Instant::now();
    let gallery = |id: u32, title: &str| {
        parse_gallery(&FetchedDocument {
        final_url: format!("https://redirected.example/g/{id}/"),
        body: format!(r#"<div id="info"><h3 id="gallery_id">#{id}</h3><h1 class="title"><span class="pretty">{title}</span></h1></div><div id="cover"><a><img src="cover.jpg"></a></div><div class="tags"><time datetime="2018-03-20T11:24:45.000Z"></time></div>"#),
    }, &format!("/g/{id}")).unwrap()
    };
    let mut cache = GalleryCache::default();
    for id in 1..=4 {
        cache.insert(format!("gallery-{id}"), gallery(id, "Title"), now);
    }
    let cached = cache
        .get("gallery-1", now + Duration::from_secs(10))
        .unwrap();
    let detail = cached.clone().into_manga("/g/1/".into());
    let chapter = cached.into_chapter("/g/1".into());
    assert_eq!(detail.path, "/g/1/");
    assert_eq!(chapter.path, "/g/1");
    assert_eq!(detail.cover_url, "https://redirected.example/g/1/cover.jpg");
    assert_eq!(chapter.number, 1.0);
    assert_eq!(chapter.uploaded, 1_521_545_085);
    cache.insert(
        "gallery-5".into(),
        gallery(5, "Fifth"),
        now + Duration::from_secs(10),
    );
    assert!(
        cache
            .get("gallery-2", now + Duration::from_secs(10))
            .is_none()
    );
    // Access promotes an entry without extending its original TTL.
    assert!(
        cache
            .get("gallery-1", now + Duration::from_secs(15))
            .is_some()
    );
    assert!(
        cache
            .get(
                "gallery-1",
                now + Duration::from_secs(15) + Duration::from_nanos(1)
            )
            .is_none()
    );
    cache.insert(
        "gallery-5".into(),
        gallery(5, "Replacement"),
        now + Duration::from_secs(20),
    );
    assert_eq!(
        cache
            .get("gallery-5", now + Duration::from_secs(30))
            .unwrap()
            .into_manga("/g/5".into())
            .title,
        "Replacement"
    );
    assert!(
        cache
            .get("gallery-5", now + Duration::from_secs(36))
            .is_none()
    );

    let mut cdn = CdnConfigCache::default();
    cdn.insert(url::Url::parse("https://cdn.example/").unwrap(), now);
    assert!(cdn.get(now + Duration::from_secs(6 * 60 * 60)).is_some());
    assert!(
        cdn.get(now + Duration::from_secs(6 * 60 * 60) + Duration::from_nanos(1))
            .is_none()
    );
}

#[test]
fn gallery_api_page_numbers_sort_cdn_urls() {
    let gallery = GalleryApiResponse {
        media_id: "123".to_string(),
        pages: vec![
            GalleryApiPage {
                number: 2,
                path: "galleries/123/2.webp".to_string(),
            },
            GalleryApiPage {
                number: 1,
                path: "galleries/123/1.jpg".to_string(),
            },
        ],
    };

    assert_eq!(
        build_gallery_page_urls(
            "456",
            &gallery,
            &url::Url::parse("https://i2.nhentai.net/").unwrap()
        )
        .unwrap(),
        vec![
            "https://i2.nhentai.net/galleries/123/1.jpg",
            "https://i2.nhentai.net/galleries/123/2.webp",
        ]
    );
}

#[test]
fn parse_nhentai_metadata_text_and_timestamp() {
    let response = FetchedDocument {
        final_url: "https://nhentai.net/g/12/".into(),
        body: r#"<div id="info"><h3 id="gallery_id">#12</h3><h1 class="title"><span class="pretty">Gallery title</span></h1></div>
            <a href="/parody/a"><span class="name">First</span></a><a href="/parody/b"><span class="name">Second</span></a>
            <a href="/character/a"><span class="name">Character</span></a><a href="/language/en"><span class="name">English</span></a>
            <a href="/category/a"><span class="name">Manga</span></a><a href="/search/?q=pages:2"><span class="name">2</span></a>
            <a href="/artist/a"><span class="name">Jane <b>Doe</b></span></a><a href="/artist/a"><span class="name">Jane Doe</span></a>"#.into(),
    };
    let detail = parse_gallery(&response, "/g/12")
        .unwrap()
        .into_manga("/g/12".into());
    assert_eq!(
        detail.description.as_deref(),
        Some(
            "#12\nParodies: First,Second\nCharacters: Character\nLanguages: English\nCategories: Manga\nPages: 2"
        )
    );
    assert_eq!(detail.author, ["Jane Doe"]);
    for path in ["/g/12/", "/g/12/?ref=listing"] {
        let listing = FetchedDocument {
            final_url: "https://nhentai.net/".into(),
            body: format!(
                r#"<div class="gallery"><a href="{path}"><div class="caption">Gallery title</div></a></div>"#
            ),
        };
        let listed = super::parse::parse_manga_list(&listing, &listing.final_url).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, path);
        assert!(parse_gallery(&response, &listed[0].path).is_ok());
    }
    // Only a matching gallery with a title can produce the cache's typed payload.
    assert!(parse_gallery(&response, "/g/13").is_err());
    let missing_title = FetchedDocument {
        body: response.body.replace("Gallery title", " "),
        ..response.clone()
    };
    assert!(parse_gallery(&missing_title, "/g/12").is_err());
    let missing_id = FetchedDocument {
        body: response
            .body
            .replace("id=\"gallery_id\"", "id=\"unrelated\""),
        ..response
    };
    assert!(parse_gallery(&missing_id, "/g/12").is_err());

    let document = Html::parse_document(
        r#"<span class="name"><span class="community"> </span> original </span>"#,
    );
    let selector = Selector::parse(".name").unwrap();

    assert_eq!(
        document
            .select(&selector)
            .next()
            .and_then(trimmed_element_text),
        Some("original".to_string())
    );
    assert_eq!(
        parse_uploaded_timestamp("2018-03-20T11:24:45.000Z"),
        Some(1_521_545_085)
    );
}

#[test]
fn parse_flaresolverr_wrapped_api_response() {
    let response: GalleryApiResponse = parse_browser_json(
        r#"<html><body><pre>{"media_id":"123","pages":[{"number":1,"path":"galleries/123/1.jpg"}]}</pre></body></html>"#,
        "NHentai gallery",
    )
    .unwrap();

    assert_eq!(response.media_id, "123");
    assert_eq!(response.pages[0].path, "galleries/123/1.jpg");
}

#[test]
#[ignore = "live source check"]
fn test_get_popular_manga() {
    let nhentai: NHentai = create_test_instance();

    let res = nhentai.get_popular_manga(1).unwrap();
    assert!(!res.is_empty());
}

#[test]
#[ignore = "live source check"]
fn test_get_latest_manga() {
    std::thread::sleep(std::time::Duration::from_secs(1));

    let nhentai: NHentai = create_test_instance();

    let res = nhentai.get_latest_manga(1).unwrap();
    assert!(!res.is_empty());
}

#[test]
#[ignore = "live source check"]
fn test_search_manga() {
    std::thread::sleep(std::time::Duration::from_secs(2));

    let nhentai: NHentai = create_test_instance();

    let res = nhentai
        .search_manga(1, Some("azur lane".to_string()), None)
        .unwrap();
    assert!(!res.is_empty());
}

#[test]
#[ignore = "live source check"]
fn test_search_manga_filter() {
    std::thread::sleep(std::time::Duration::from_secs(3));

    let nhentai: NHentai = create_test_instance();

    let mut filters = nhentai.filter_list();
    for filter in filters.iter_mut() {
        if SORT_FILTER.eq(filter)
            && let Input::Select { state, .. } = filter
        {
            *state = Some(1);
        } else if TAG_FILTER.eq(filter)
            && let Input::Text { state, .. } = filter
        {
            *state = Some("-big breasts".to_string());
        } else if PARODIES_FILTER.eq(filter)
            && let Input::Text { state, .. } = filter
        {
            *state = Some("azur-lane".to_string());
        }
    }
    let res = nhentai.search_manga(1, None, Some(filters)).unwrap();
    assert!(!res.is_empty());
}

#[test]
#[ignore = "live source check"]
fn test_get_manga_detail() {
    let nhentai: NHentai = create_test_instance();

    let res = nhentai.get_manga_detail("/g/385965".to_string()).unwrap();

    assert_eq!(res.title, "Lady, Maid ni datsu");
    assert!(res.genre.iter().all(|tag| !tag.trim().is_empty()));
}

#[test]
#[ignore = "live source check"]
fn test_get_chapters() {
    std::thread::sleep(std::time::Duration::from_secs(1));

    let nhentai: NHentai = create_test_instance();

    let res = nhentai.get_chapters("/g/385965".to_string()).unwrap();
    // A gallery is a complete work exposed as exactly one synthetic chapter.
    assert_eq!(res.len(), 1, "a gallery must contain exactly one chapter");
    assert_eq!(res[0].path, "/g/385965");
    assert_eq!(res[0].number, 1.0);
    assert!(res.iter().all(|chapter| chapter.uploaded > 0));
}

#[test]
#[ignore = "live source check"]
fn test_get_pages() {
    std::thread::sleep(std::time::Duration::from_secs(2));

    let nhentai: NHentai = create_test_instance();

    let page = "/g/385965".to_string();
    let res = nhentai.get_pages(page).unwrap();
    assert!(!res.is_empty());
    assert!(res[0].starts_with("https://i"));
    assert!(res[0].ends_with("/galleries/2099700/1.jpg"));
    extension_utils::assert_valid_page_image(&nhentai, &res[0]);

    let page = "/g/624576".to_string();
    let res = nhentai.get_pages(page).unwrap();
    assert!(!res.is_empty());
    assert!(res[1].starts_with("https://i"));
    assert!(res[1].ends_with("/galleries/3748415/2.webp"));
    extension_utils::assert_valid_page_image(&nhentai, &res[1]);
}
