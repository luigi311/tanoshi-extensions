use anyhow::{Context, Result, ensure};
use url::Url;

/// Resolve a page's asset reference against the final document URL.
/// Assets may live on another host; this must not be used for stored identities.
pub fn resolve_asset_url(page_url: &str, reference: &str) -> Result<String> {
    let reference = reference.trim();
    ensure!(!reference.is_empty(), "empty asset reference");
    let base = Url::parse(page_url).context("invalid document URL")?;
    ensure!(
        matches!(base.scheme(), "http" | "https") && base.host_str().is_some(),
        "document URL must use HTTP(S) with a host"
    );
    let mut asset = base.join(reference).context("invalid asset reference")?;
    ensure!(
        matches!(asset.scheme(), "http" | "https") && asset.host_str().is_some(),
        "asset URL must use HTTP(S) with a host"
    );
    asset.set_fragment(None);
    Ok(asset.into())
}

/// Resolve a website link into a stored path, trusting the configured source
/// and this response's final origin. Redirect trust is scoped to this response;
/// it does not authorize unrelated hosts or change the configured fetch origin.
pub fn source_link_path(source_url: &str, page_url: &str, reference: &str) -> Result<String> {
    let reference = reference.trim();
    ensure!(!reference.is_empty(), "empty source link");
    let source = Url::parse(source_url).context("invalid source URL")?;
    let page = Url::parse(page_url).context("invalid document URL")?;
    let link = page.join(reference).context("invalid source link")?;
    for url in [&source, &page, &link] {
        ensure!(
            matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
            "source links must use HTTP(S) with a host"
        );
        ensure!(
            url.username().is_empty() && url.password().is_none(),
            "source links must not contain credentials"
        );
    }
    ensure!(
        link.origin() == source.origin() || link.origin() == page.origin(),
        "source link belongs to an unexpected origin: {}",
        link.origin().ascii_serialization()
    );
    ensure!(
        !link.path().trim_matches('/').is_empty(),
        "source link has no identity path"
    );
    // URL slices retain escaped segments, trailing slashes, and the query.
    // A fragment is a page location, not a manga/chapter identity.
    Ok(link[url::Position::BeforePath..url::Position::AfterQuery].to_string())
}

/// Validate a stored, root-relative identity before building a request URL.
/// The returned URL is for transport only: callers retain the stored identity.
/// Route and identifier rules belong to each source, not this structural check.
pub fn source_request_url(source_url: &str, path: &str) -> Result<Url> {
    ensure!(
        path.starts_with('/') && !path.starts_with("//"),
        "stored source path must start with a single slash: {path:?}"
    );
    ensure!(
        !path.contains('\\') && !path.chars().any(char::is_control),
        "stored source path contains a backslash or control character: {path:?}"
    );
    let base = Url::parse(source_url).context("invalid source URL")?;
    ensure!(
        matches!(base.scheme(), "http" | "https") && base.host_str().is_some(),
        "source URL must use HTTP(S) with a host"
    );
    let mut request = base.join(path).context("invalid stored source path")?;
    ensure!(
        request.origin() == base.origin()
            && request.username().is_empty()
            && request.password().is_none(),
        "stored source path changes the source authority"
    );
    ensure!(
        !request.path().trim_matches('/').is_empty(),
        "stored source path has no identity"
    );
    request.set_fragment(None);
    Ok(request)
}

/// Resolve optional artwork, using the host's empty-string representation when absent.
pub fn resolve_cover_url(page_url: &str, reference: Option<&str>) -> String {
    let Some(reference) = reference.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };
    match resolve_asset_url(page_url, reference) {
        Ok(url) => url,
        Err(error) => {
            log::warn!("Ignoring invalid cover {reference:?} from {page_url}: {error:#}");
            String::new()
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn asset_references_use_document_base_and_preserve_queries() {
        let base = "https://reader.example/redirected/chapter/?lang=en";
        for (reference, expected) in [
            ("1.jpg", "https://reader.example/redirected/chapter/1.jpg"),
            ("../1.jpg", "https://reader.example/redirected/1.jpg"),
            (" /images/1.jpg \n", "https://reader.example/images/1.jpg"),
            ("//cdn.example/1.jpg", "https://cdn.example/1.jpg"),
            ("http://cdn.example/1.jpg", "http://cdn.example/1.jpg"),
            (
                "1.jpg?token=a%2Fb#preview",
                "https://reader.example/redirected/chapter/1.jpg?token=a%2Fb",
            ),
            (
                "/image?from=https://reader.example/a",
                "https://reader.example/image?from=https://reader.example/a",
            ),
            ("https://cdn.example/folder/", "https://cdn.example/folder/"),
        ] {
            assert_eq!(resolve_asset_url(base, reference).unwrap(), expected);
        }
        for reference in [
            "",
            " \n",
            "data:image/png;base64,AA",
            "javascript:void(0)",
            "file:///image.png",
        ] {
            assert!(resolve_asset_url(base, reference).is_err(), "{reference}");
        }
        assert!(resolve_asset_url("/relative", "1.jpg").is_err());
    }
    #[test]
    fn source_links_trust_only_configured_and_final_origins() {
        let source = "https://old.example";
        let page = "https://new.example/catalog/page/";
        for (reference, expected) in [
            ("https://old.example/manga/a/", "/manga/a/"),
            ("https://new.example/manga/a", "/manga/a"),
            ("//new.example/manga/a", "/manga/a"),
            (
                " /manga/a?from=https://old.example/a#chapter ",
                "/manga/a?from=https://old.example/a",
            ),
            ("../a%2Fb?token=x%2Fy", "/catalog/a%2Fb?token=x%2Fy"),
        ] {
            assert_eq!(source_link_path(source, page, reference).unwrap(), expected);
        }
        for reference in [
            "",
            "/",
            "https://foreign.example/manga/a",
            "//foreign.example/manga/a",
            "https://new.example:444/manga/a",
            "https://old.example.foreign.example/manga/a",
            "https://user@new.example/manga/a",
            "javascript:void(0)",
        ] {
            assert!(
                source_link_path(source, page, reference).is_err(),
                "{reference}"
            );
        }
        // A previously seen redirect does not widen trust for another response.
        assert!(source_link_path(source, source, "https://new.example/manga/a").is_err());
    }
    #[test]
    fn stored_paths_keep_existing_identity_forms_and_query_boundaries() {
        let source = "https://source.example";
        for path in [
            "/webtoon/ooh-la-la",
            "/webtoon/ooh-la-la/",
            "/manga/title/chapter-1/",
            "/api/series/Kaguya-Wants-To-Be-Confessed-To/1",
            "/api/series/title/1.5",
            "/g/385965",
            "/g/385965/",
            "/series/01J76XY8K8BPR60XQNGPTEJ767",
            "/chapters/01JDHRNVEN6TES6S327K0AFXY8",
            "/manga/legacy-id",
            "/chapter/legacy-id",
            "/series/a%2Fb/?from=https://source.example/a&token=x%2Fy",
        ] {
            assert_eq!(
                source_request_url(source, path).unwrap().as_str(),
                format!("{source}{path}")
            );
        }
        let mut request = source_request_url(source, "/series/a%2Fb/?token=x%2Fy#reader").unwrap();
        request.set_path(&format!(
            "{}/full-chapter-list",
            request.path().trim_end_matches('/')
        ));
        assert_eq!(
            request.as_str(),
            "https://source.example/series/a%2Fb/full-chapter-list?token=x%2Fy"
        );
        for path in [
            "",
            "relative",
            "https://foreign.example/manga/a",
            "//foreign.example/manga/a",
            "/\\foreign.example/a",
            "/a\nb",
            "/",
            "/?query=1",
        ] {
            assert!(source_request_url(source, path).is_err(), "{path:?}");
        }
    }
}
