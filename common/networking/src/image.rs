use crate::client::HttpResponse;
use anyhow::{Context, Result, anyhow, ensure};
use bytes::Bytes;
use scraper::{Html, Selector};
use std::collections::HashSet;
use ureq::{ResponseExt, http::header::CONTENT_TYPE, typestate::WithoutBody};
use url::Url;

const LIMIT_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB

const IMAGE_ACCEPT: &str = "image/avif,image/webp,image/apng,image/png,image/jpeg,image/gif,image/bmp,application/octet-stream;q=0.8";

pub(crate) fn build_image_get_with_referer(
    url: &str,
    mut req: ureq::RequestBuilder<WithoutBody>,
    referer: Option<&str>,
) -> ureq::RequestBuilder<WithoutBody> {
    let referer = referer
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            Url::parse(url)
                .ok()
                .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
                .map(|u| format!("{}/", u.origin().ascii_serialization()))
        });

    req = req.header("Accept", IMAGE_ACCEPT);

    if let Some(r) = referer {
        req = req.header("Referer", r);
    }

    req
}

/// Whether an image request may follow an HTML page's first image link.
/// Enable only for a source known to return image wrapper pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageWrapperPolicy {
    #[default]
    Reject,
    FirstImage,
}

/// A parsed image response: either the image bytes, or the URL of the real
/// image when the server answered with an HTML wrapper page.
#[derive(Debug)]
pub(crate) enum ImageResponse {
    Bytes(Bytes),
    Redirect { from: Url, to: Url },
}

/// Validate status and content type, then read the body: image bytes pass
/// through; explicitly allowed HTML wrappers resolve against the final URL.
pub(crate) fn parse_image_response(
    mut resp: HttpResponse,
    url: &str,
    wrappers: ImageWrapperPolicy,
) -> Result<ImageResponse> {
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let is_html = matches!(content_type.as_str(), "text/html" | "application/xhtml+xml");
    let data = resp
        .body_mut()
        .with_config()
        .limit(if is_html {
            10 * 1024 * 1024
        } else {
            LIMIT_BYTES
        })
        .read_to_vec()?;
    let recognized = has_image_signature(&data);
    if !recognized
        && crate::flaresolverr::cf_challenge_marker(
            status.as_u16(),
            &String::from_utf8_lossy(&data),
        )
        .is_some()
    {
        return Err(anyhow::Error::new(ImageChallenge).context(format!(
            "HTTP {} while fetching image {url}",
            status.as_u16()
        )));
    }
    ensure!(
        status.as_u16() < 400,
        "Image fetch failed: HTTP {} for {url}",
        status.as_u16()
    );

    if is_html {
        ensure!(
            wrappers == ImageWrapperPolicy::FirstImage,
            "Expected image bytes but got HTML for {url}; HTML wrappers are disabled for this source"
        );
        let from = image_url(&resp.get_uri().to_string())?;
        let html = String::from_utf8_lossy(&data);
        let next = extract_first_img_src(&html)
            .filter(|src| !src.trim().is_empty())
            .ok_or_else(|| anyhow!("Image wrapper has no nonempty first <img src> for {from}"))?;
        let to = image_url(from.join(next.trim())?.as_str())?;
        return Ok(ImageResponse::Redirect { from, to });
    }

    ensure!(
        matches!(
            content_type.as_str(),
            "" | "application/octet-stream"
                | "binary/octet-stream"
                | "application/binary"
                | "image/jpeg"
                | "image/jpg"
                | "image/png"
                | "image/apng"
                | "image/gif"
                | "image/webp"
                | "image/bmp"
                | "image/x-bmp"
                | "image/x-ms-bmp"
                | "image/avif"
                | "image/avif-sequence"
        ),
        "Expected image bytes but got unsupported Content-Type {content_type:?} for {url}"
    );
    ensure!(
        recognized,
        "Empty, unsupported or invalid image content for {url}"
    );
    Ok(ImageResponse::Bytes(Bytes::from(data)))
}

/// One traversal shared by direct and solver-aware request policies.
pub(crate) fn bytes_fetch_impl<F>(do_get: &mut F, url: &str) -> Result<Bytes>
where
    F: FnMut(&str) -> Result<ImageResponse>,
{
    let mut current = image_url(url)?;
    let mut visited = HashSet::new();
    for _ in 0..=2 {
        ensure!(
            visited.insert(current.clone()),
            "Image wrapper cycle at {current}"
        );
        match do_get(current.as_str())? {
            ImageResponse::Bytes(bytes) => return Ok(bytes),
            ImageResponse::Redirect { from, to } => {
                if from != current {
                    ensure!(
                        visited.insert(from.clone()),
                        "Image wrapper cycle at {from}"
                    );
                }
                ensure!(!visited.contains(&to), "Image wrapper cycle at {to}");
                current = to;
            }
        }
    }
    Err(anyhow!("Too many wrapper hops while fetching image: {url}"))
}

#[derive(Debug)]
pub(crate) struct ImageChallenge;
impl std::fmt::Display for ImageChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cloudflare challenge returned instead of image bytes")
    }
}
impl std::error::Error for ImageChallenge {}

// Format recognition only: full decoding remains in the dev-only live helper.
fn has_image_signature(data: &[u8]) -> bool {
    data.starts_with(b"\x89PNG\r\n\x1a\n")
        || data.starts_with(b"\xff\xd8\xff")
        || data.starts_with(b"GIF87a")
        || data.starts_with(b"GIF89a")
        || (data.len() >= 26 && data.starts_with(b"BM"))
        || (data.starts_with(b"RIFF")
            && data.get(8..12) == Some(b"WEBP")
            && matches!(data.get(12..16), Some(b"VP8 " | b"VP8L" | b"VP8X")))
        || is_avif(data)
}

fn is_avif(data: &[u8]) -> bool {
    if data.get(4..8) != Some(b"ftyp") {
        return false;
    }
    let length = u32::from_be_bytes(data[..4].try_into().unwrap());
    let (start, end) = match length {
        0 => (8, data.len()),
        1 => {
            let Some(bytes) = data.get(8..16) else {
                return false;
            };
            let Ok(end) = usize::try_from(u64::from_be_bytes(bytes.try_into().unwrap())) else {
                return false;
            };
            (16, end)
        }
        length => (8, length as usize),
    };
    let Some(brands) = data.get(start..end) else {
        return false;
    };
    // ftyp contains major brand, minor version, then aligned compatible brands.
    if brands.len() < 8 || brands.len() % 4 != 0 {
        return false;
    }
    brands
        .chunks_exact(4)
        .enumerate()
        .any(|(index, brand)| index != 1 && matches!(brand, b"avif" | b"avis"))
}

fn image_url(value: &str) -> Result<Url> {
    let mut url = Url::parse(value).context("Invalid image URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "Image URL must use HTTP(S)"
    );
    url.set_fragment(None);
    Ok(url)
}

// Tiny helper: pull the first <img ... src="..."> out of wrapper HTML.
fn extract_first_img_src(html: &str) -> Option<String> {
    let selector = Selector::parse("img[src]").ok()?;
    Html::parse_document(html)
        .select(&selector)
        .find_map(|image| image.value().attr("src").map(str::to_string))
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn image_validation_rejects_error_bodies_and_recognizes_binary_images() {
        fn parse(mime: &str, data: &[u8]) -> Result<ImageResponse> {
            let response = ureq::http::Response::builder()
                .header(CONTENT_TYPE, mime)
                .body(ureq::Body::builder().data(data.to_vec()))
                .unwrap();
            parse_image_response(
                response,
                "https://images.test/page",
                ImageWrapperPolicy::Reject,
            )
        }
        // These exercise signature recognition, not full decoding. Existing
        // source checks fully decode sampled images using the dev-only helper.
        for data in [
            &b"\xff\xd8\xff"[..],
            &b"\x89PNG\r\n\x1a\n"[..],
            &b"GIF89a"[..],
            &b"RIFFxxxxWEBPVP8 "[..],
            &b"BM000000000000000000000000"[..],
        ] {
            for mime in [
                "application/octet-stream",
                "binary/octet-stream",
                "application/binary",
                "",
            ] {
                assert!(
                    matches!(parse(mime, data), Ok(ImageResponse::Bytes(bytes)) if bytes.as_ref() == data)
                );
            }
        }
        for (mime, data) in [
            ("image/png", &b"{\"error\":\"missing\"}"[..]),
            ("application/octet-stream", &b"access denied"[..]),
            ("application/json", &b"{\"error\":true}"[..]),
            ("text/plain", &b"not an image"[..]),
            ("image/gif", &b""[..]),
            ("application/octet-stream", &b"RIFFxxxxWAVE"[..]),
        ] {
            assert!(parse(mime, data).is_err(), "{mime}");
        }
        for mime in ["text/html", "image/png", "application/octet-stream"] {
            let error = parse(
                mime,
                b"<html>Cloudflare: Just a moment<img src='/logo.png'></html>",
            )
            .unwrap_err();
            assert!(error.is::<ImageChallenge>());
        }
    }

    #[test]
    fn test_extract_img_src_basic() {
        let html = r#"<html><body><img src="https://example.com/image.png"></body></html>"#;
        assert_eq!(
            extract_first_img_src(html),
            Some("https://example.com/image.png".to_string())
        );
    }
    #[test]
    fn test_extract_img_src_relative() {
        let html = r#"<img src="/images/foo.jpg" alt="foo">"#;
        assert_eq!(
            extract_first_img_src(html),
            Some("/images/foo.jpg".to_string())
        );
    }
    #[test]
    fn test_extract_img_src_no_img() {
        let html = "<html><body>No images here</body></html>";
        assert_eq!(extract_first_img_src(html), None);
    }
    #[test]
    fn test_extract_img_src_picks_first() {
        let html = r#"
            <script src="not-an-image.js"></script>
            <iframe src="not-an-image.html"></iframe>
            <img src="first.png"><img src="second.png">
        "#;
        assert_eq!(extract_first_img_src(html), Some("first.png".to_string()));
    }
    #[test]
    fn test_build_image_get_sets_accept_header() {
        let agent = crate::client::build_ureq_agent(None);
        for (url, referer, expected) in [
            (
                "https://cdn.example.com/image.png",
                None,
                Some("https://cdn.example.com/"),
            ),
            (
                "http://localhost:8080/image.png",
                None,
                Some("http://localhost:8080/"),
            ),
            (
                "https://user:pass@[::1]:8443/image.png",
                None,
                Some("https://[::1]:8443/"),
            ),
            (
                "https://cdn.example.com/image.png",
                Some("https://source.example/chapter"),
                Some("https://source.example/chapter"),
            ),
            ("invalid", None, None),
            ("ftp://cdn.example.com/image.png", None, None),
        ] {
            let req = build_image_get_with_referer(
                url,
                agent.get("https://cdn.example.com/image.png"),
                referer,
            );
            let headers = req.headers_ref().unwrap();
            assert_eq!(headers.get("Accept").unwrap(), IMAGE_ACCEPT);
            assert_eq!(
                headers.get("Referer").map(|value| value.to_str().unwrap()),
                expected
            );
        }
    }
}

#[cfg(test)]
#[test]
fn wrapper_policy_is_shared_and_uses_final_urls_with_bounded_traversal() {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };

    // A small scripted server checks the requested paths and source referer.
    // The timeout also makes unexpected/missing wrapper requests fail the test.
    fn run(solver: bool, allow: bool, steps: &[(&str, &str, &str)], error: Option<&str>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let referer = format!("{origin}/source/");
        let expected_referer = referer.clone();
        let steps: Vec<_> = steps
            .iter()
            .map(|(p, h, b)| (p.to_string(), h.to_string(), b.to_string()))
            .collect();
        let worker = std::thread::spawn(move || {
            for (path, headers, body) in steps {
                let deadline = Instant::now() + Duration::from_secs(5);
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "missing request for {path}");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                assert!(
                    request.starts_with(&format!("GET {path} HTTP/1.1\r\n")),
                    "{request}"
                );
                assert!(request.lines().any(|line| {
                    line.split_once(':').is_some_and(|(name, value)| {
                        name.eq_ignore_ascii_case("referer") && value.trim() == expected_referer
                    })
                }));
                let response = format!(
                    "HTTP/1.1 {headers}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                // Rejected wrappers can close the connection without reading the body.
                let _ = reader.get_mut().write_all(response.as_bytes());
            }
        });
        let policy = if allow {
            ImageWrapperPolicy::FirstImage
        } else {
            ImageWrapperPolicy::Reject
        };
        let url = format!("{origin}/start");
        let result = if solver {
            let client = crate::FlareClient::new(crate::FlareClientConfig {
                origin_url: referer,
                ..Default::default()
            });
            if allow {
                client.fetch_bytes_with_wrapper_policy(&url, policy)
            } else {
                client.fetch_bytes(&url)
            }
        } else {
            let client = crate::build_rate_limited_ureq_agent(None, None);
            if allow {
                client.fetch_bytes_with_wrapper_policy(&url, Some(&referer), policy)
            } else {
                client.fetch_bytes(&url, Some(&referer))
            }
        };
        worker.join().unwrap();
        if let Some(error) = error {
            assert!(format!("{:#}", result.unwrap_err()).contains(error));
        } else {
            assert_eq!(result.unwrap().as_ref(), b"GIF89a");
        }
    }

    let html = "200 OK\r\nContent-Type: text/html";
    for solver in [false, true] {
        run(
            solver,
            false,
            &[("/start", html, "<img src='/page.gif'>")],
            Some("wrappers are disabled"),
        );
        run(
            solver,
            true,
            &[
                ("/start", "302 Found\r\nLocation: /moved/wrapper", ""),
                ("/moved/wrapper", html, "<img src=' page.gif '>"),
                (
                    "/moved/page.gif",
                    "200 OK\r\nContent-Type: image/gif",
                    "GIF89a",
                ),
            ],
            None,
        );
        run(
            solver,
            true,
            &[("/start", html, "<img src=' '> <img src='/other.gif'>")],
            Some("nonempty"),
        );
        run(
            solver,
            true,
            &[("/start", html, "<img src='#fragment'>")],
            Some("cycle"),
        );
        run(
            solver,
            true,
            &[
                ("/start", html, "<img src='/next'>"),
                ("/next", html, "<img src='/start'>"),
            ],
            Some("cycle"),
        );
        run(
            solver,
            true,
            &[
                ("/start", html, "<img src='/one'>"),
                ("/one", html, "<img src='/two'>"),
                ("/two", html, "<img src='/three'>"),
            ],
            Some("Too many wrapper hops"),
        );
    }
}
