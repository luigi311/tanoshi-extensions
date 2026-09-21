use super::parse::{parse_chapter_number, source_id};
use super::*;

#[test]
fn metadata_labels_agree_across_list_and_detail_when_reordered() {
    for genre_label in ["Tag(s)", "Tags(s)", "Genre(s)", "Genres"] {
        let listing = networking::FetchedDocument {
            final_url: format!("{URL}/search/data"),
            body: format!(
                r#"<article class="bg-base-300">
                <a href="{MANGA_PATH}"><div class="text-ellipsis truncate">Planetes</div></a>
                <div class="opacity-70"><strong>Status:</strong><span><b>Complete</b></span></div>
                <div class="opacity-70"><strong>Year:</strong><span>1999</span></div>
                <div class="opacity-70"><strong>{genre_label}: </strong><span>Sci-fi</span></div>
            </article>"#
            ),
        };
        let detail = networking::FetchedDocument {
            final_url: format!("{URL}{MANGA_PATH}"),
            body: format!(
                r#"<h1 class="hidden md:block text-2xl font-bold">Planetes</h1>
                <ul class="flex flex-col gap-4">
                    <li><strong>{genre_label}: </strong><span><a class="link link-info link-hover">Sci-fi</a></span></li>
                    <li><strong>Status:</strong><a class="link link-info link-hover"><b>Complete</b></a></li>
                    <li><strong>Year:</strong><span>1999</span></li>
                </ul>"#
            ),
        };
        let list = parse::parse_manga_list(&listing, &listing.final_url).unwrap();
        let detail = parse::parse_detail(&detail, MANGA_PATH.into()).unwrap();
        assert_eq!(list.len(), 1);
        for manga in [&list[0], &detail] {
            assert_eq!(manga.status.as_deref(), Some("Complete"), "{genre_label}");
            assert_eq!(manga.genre, ["Sci-fi"], "{genre_label}");
        }
    }
}

#[test]
fn parse_chapter_number_handles_decimals_and_specials() {
    assert_eq!(parse_chapter_number("Chapter 12.5"), Some(12.5));
    assert_eq!(parse_chapter_number("chapter 7 - The Return"), Some(7.0));
    assert_eq!(parse_chapter_number("Special 1"), None);
}

#[test]
fn source_id_handles_relative_and_absolute_links() {
    for marker in ["chapters", "series"] {
        for id in ["01JDHRNVEN6TES6S327K0AFXY8", "01JDHRNVENFGM20AY21CWXTJJE"] {
            let path = format!("/{marker}/{id}");
            for link in [
                path.clone(),
                format!("{URL}{path}"),
                format!("{path}?page=2"),
                format!("{path}#reader"),
                format!("https://redirected.example{path}"),
                format!("//redirected.example{path}"),
            ] {
                assert_eq!(
                    source_id(&link, marker, "https://redirected.example/").unwrap(),
                    id
                );
            }
        }
    }
}

#[test]
fn source_id_rejects_missing_or_foreign_ids() {
    for marker in ["chapters", "series"] {
        for link in [
            String::new(),
            format!("/{marker}/"),
            format!("/{marker}/?page=1"),
            format!("{URL}/{marker}/"),
            "/unrelated/id".to_string(),
            format!("https://foreign.example/{marker}/id"),
            format!("/unrelated/{marker}/id"),
            format!("/{marker}/#reader"),
        ] {
            assert!(source_id(&link, marker, URL).is_err(), "accepted {link:?}");
        }
    }
}

// Planetes: completed series, so the values asserted below are stable.
const MANGA_PATH: &str = "/series/01J76XY8K8BPR60XQNGPTEJ767";

fn create_test_instance() -> Weebcentral {
    Weebcentral::default()
}

#[test]
#[ignore = "live source check"]
fn test_get_latest_manga() {
    let weebcentral = create_test_instance();

    let res1 = weebcentral.get_latest_manga(1).unwrap();
    assert!(!res1.is_empty());

    let res2 = weebcentral.get_latest_manga(2).unwrap();
    assert!(!res2.is_empty());

    assert_ne!(
        res1[0].path, res2[0].path,
        "{} should be different than {}",
        res1[0].path, res2[0].path
    );
}

#[test]
#[ignore = "live source check"]
fn test_get_popular_manga() {
    let weebcentral = create_test_instance();

    let res = weebcentral.get_popular_manga(1).unwrap();
    assert!(!res.is_empty());
}

#[test]
#[ignore = "live source check"]
fn test_get_popular_manga_past_end_is_empty_not_error() {
    // The site's "No results found" alert past the last page is a
    // legitimate empty result, not a markup-change error.
    let weebcentral = create_test_instance();

    let res = weebcentral.get_popular_manga(99999).unwrap();
    assert!(res.is_empty());
}

#[test]
#[ignore = "live source check"]
fn test_search_manga() {
    let weebcentral = create_test_instance();

    let res = weebcentral
        .search_manga(1, Some("planetes".to_string()), None)
        .unwrap();

    assert!(!res.is_empty());
    assert!(
        res.iter().any(|m| m.path == MANGA_PATH),
        "search results should contain {}, got {:?}",
        MANGA_PATH,
        res.iter().map(|m| m.path.clone()).collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "live source check"]
fn test_get_manga_detail() {
    let weebcentral = create_test_instance();

    let res = weebcentral
        .get_manga_detail(MANGA_PATH.to_string())
        .unwrap();

    assert_eq!(res.title, "Planetes");

    // The fields below come from sidebar sections located by their
    // <strong> label text, so they double as label-parsing tests.
    assert!(
        res.author.iter().any(|a| a.contains("Makoto")),
        "author should be parsed from the Author(s) section, got {:?}",
        res.author
    );
    assert!(
        res.genre.iter().any(|g| g == "Sci-fi"),
        "genre should be parsed from the Tags(s) section, got {:?}",
        res.genre
    );
    assert_eq!(
        res.status.as_deref(),
        Some("Complete"),
        "status should be parsed from the Status section"
    );
}

#[test]
#[ignore = "live source check"]
fn test_get_chapters() {
    let weebcentral = create_test_instance();

    let res = weebcentral.get_chapters(MANGA_PATH.to_string()).unwrap();

    assert_eq!(res.len(), 26, "Planetes should have all 26 chapters");
    assert!(
        res.iter().all(|c| c
            .path
            .strip_prefix("/chapters/")
            .is_some_and(|id| !id.is_empty())),
        "chapter paths should look like /chapters/<id> with a non-empty id, got {:?}",
        res.iter().map(|c| c.path.clone()).collect::<Vec<_>>()
    );
    let unique_paths: std::collections::HashSet<_> = res.iter().map(|c| &c.path).collect();
    assert_eq!(
        unique_paths.len(),
        res.len(),
        "chapter paths must be unique"
    );
    let uploaded_count = res.iter().filter(|c| c.uploaded > 0).count();
    assert!(
        uploaded_count * 2 >= res.len(),
        "at least half of upload dates should parse instead of falling back to epoch 0; got {uploaded_count}/{}",
        res.len()
    );
}

#[test]
#[ignore = "live source check"]
fn test_get_pages() {
    let weebcentral = create_test_instance();

    let chapters = weebcentral.get_chapters(MANGA_PATH.to_string()).unwrap();
    let chapter = chapters.first().expect("chapter list should not be empty");

    let res = weebcentral.get_pages(chapter.path.clone()).unwrap();
    assert!(!res.is_empty());
    assert!(
        res.iter().all(|p| p.starts_with("http")),
        "pages should be absolute image urls, got {:?}",
        res.first()
    );
    extension_utils::assert_valid_page_image(&weebcentral, &res[0]);
}
