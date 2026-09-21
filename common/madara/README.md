# Madara protocol ownership

Madara remains shared because Manhwa18cc is active and TritiniaScans plus
365Manga/HariManga are retained restoration candidates. The latter two are
excluded reference code, not supported or buildable plugins. Their current
protocols must be verified before restoration; retaining these functions does
not establish that the websites still accept the historical requests.

`api.rs` owns requests, clients, endpoints, forms and response context.
`parse.rs` owns HTML selectors, field validation, mapping and chapter dates.
The root module exports the supported entry points.

| Entry point | Consumer / intended consumer | Protocol |
|---|---|---|
| `parse_manga_list` | Manhwa18cc and shared request helpers | Cards containing title/link/image descendants |
| `search_html` | Retained helper; no active caller | GET `/search?q=...&page=...` without source-specific pagination |
| `fetch_html_chapters` | Manhwa18cc | GET detail page; `#chapterlist .a-h.wleft` rows |
| `get_manga_detail` | Manhwa18cc; Tritinia/HariManga candidates | GET detail page; shared title/metadata selectors |
| `fetch_ajax_latest`, `fetch_ajax_popular` | Tritinia/HariManga candidates | POST `madara_load_more` to `/wp-admin/admin-ajax.php` |
| `search_ajax` | Tritinia/HariManga candidates | POST search form; `div.c-tabs-item__content` cards |
| `fetch_ajax_chapters` | Tritinia/HariManga candidates | POST `<series>/ajax/chapters`; `li.wp-manga-chapter,li.chapter-li` rows |
| `fetch_pages` | Tritinia/HariManga candidates | POST reader page; historical Madara image selectors |

Manhwa18cc owns its `/webtoons` browse and `/search` requests, pagination, and
`.read-content` page parser. Browse and paginated search return empty results
when the requested page exceeds the active final page with Next disabled.
Search responses without pagination occupy one page; later pages are empty.
Missing browse pagination or inconsistent page markers produce errors.
The shared detail parser accepts its badge-prefixed titles and
wrapped titles. Chapter dates preserve the documented zero fallback and checked
relative-date arithmetic. The retained POST reader now resolves each selected
image against the final document URL and rejects missing or invalid required
image sources. Its transport/parser improvements do not certify compatibility
with the restoration candidates; no active extension calls that reader API.

`DetailClient` is the small shared interface for direct `RateLimitedAgent` and
solver-aware `FlareClient` detail requests. Its `fetch_document` method returns
`FetchedDocument { final_url, body }`. It is not a general transport framework.
Other request helpers retain their specific client requirements.

## URL, metadata and error contracts

Requests built from stored source paths use structural path validation before
I/O. AJAX endpoint suffixes are added before query strings. List/chapter identity
links resolve against the final document URL and must belong to the configured
source origin or that response's final origin; global origin-text replacement
is no longer used. Saved identities and source-specific chapter conventions
remain separate from transport URL construction.

Relative covers and reader images resolve against the final document URL.
Assets may use independent CDN hosts but must use HTTP(S). Invalid optional
covers log a warning and become the host's empty-cover representation. Missing
required title/link fields are reported with source context: a malformed list
card can be skipped with diagnostics, while malformed selected chapter/page
records fail the operation. An unrecognized empty response is not treated as
proof that a source has no content.

Metadata extraction keeps nested author/artist names together, deduplicates
them in first-seen order and preserves paragraph/line boundaries in descriptions.
The [chapter conventions](../../docs/source-behavior.md)
describe numeric/date fallbacks. See the
[networking policy](../networking/README.md) for final URLs, solver behavior and
the recorded POST redirect limitation.

## Updating retained wrappers during restoration

The excluded wrapper source is intentionally unchanged. Its old imports map to:

| Old name | Current name |
|---|---|
| `get_latest_manga` | `fetch_ajax_latest` |
| `get_popular_manga` | `fetch_ajax_popular` |
| `search_manga` | `search_ajax` (remove the `false` selector argument) |
| `get_chapters` | `fetch_ajax_chapters` |
| `get_pages` | `fetch_pages` |

The formerly exposed `is_selector_url=true` variant had no active or retained
caller and was removed. The remaining list parser always finds a link inside
its selected card. `search_manga_old` became `search_html`, and
`get_chapters_old` became `fetch_html_chapters` in the active Manhwa18cc caller.
Restoration still requires dependency-path, host-API, client and website review.
In addition to renaming imports, adapt callers to the current signatures:
the shared list parser receives a `FetchedDocument`, and custom `DetailClient`
implementations must retain the final URL. Revalidate site protocols rather than
assuming the old wrapper becomes compatible through renamed functions alone.

## Verification

```sh
cargo test -p madara --locked
cargo check -p madara -p manhwa18cc --all-targets --locked
FLARESOLVERR_URL=http://localhost:8191/v1 cargo test -p manhwa18cc --locked -- --ignored --test-threads=1
```

The Madara tests are local. Manhwa18cc's ignored checks exercise the active
website integration, not the retained AJAX wrappers. Follow the
[live-test setup](../../docs/development.md#build-and-verify) for solver readiness and
network access when running live checks. No retained extension is made part of
the workspace by these commands.
