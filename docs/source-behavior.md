# Chapter identity, ordering and fallbacks

Chapter paths identify releases. Chapter numbers are display/sort metadata and can
repeat; distinct paths must not be merged just because their numbers match. The
host also applies its own ordering to stored chapters.

| Source | Chapter number and fallback | Upload time and fallback | Extension output order |
|--------|-----------------------------|--------------------------|------------------------|
| MangaDex | Parse the API chapter label as a number; missing or unparseable labels use `0`. | Required API `publishAt`, converted to Unix seconds; malformed or missing timestamps fail decoding. | Feed creation time ascending. Different release IDs with the same number remain separate. |
| Guya | Parse the chapter-map key as a number; unparseable keys use `0`. The complete key is encoded as one path segment and decoded once for lookup. | The selected group's release timestamp is cast to integer seconds; a missing timestamp uses `0`. | Number ascending, then path for equal numbers. |
| Manhwa18.cc / Madara | Remove the literal `Chapter ` prefix and parse the first space-separated token; failures use `0`. | Supported absolute or relative date; missing, unrecognized or out-of-range dates use `0`. | Source HTML order. |
| NHentai | A validated gallery is represented as one chapter numbered `1`, retaining the gallery path. | RFC 3339 upload time from the gallery; missing or unparseable values use `0`. | One chapter per gallery. |
| WeebCentral | Parse digits and decimal points following case-insensitive `chapter`; otherwise use matched chapter count minus the row's zero-based index. | Parse the source time as a UTC date-time; missing or unparseable values use `0`. | Source HTML order. |

Zero remains the host-compatible unknown upload-time sentinel. Madara accepts
dates such as `September 20, 2020` and `20 Sep 2020` at midnight UTC, plus relative
seconds, minutes, hours, days, weeks, months and years. Relative dates in a response
share one reference time. Months remain approximations of 30 days and years of
365 days; checked arithmetic rejects values outside the supported range.

WeebCentral's positional fallback is intentionally retained for specials. Its
number can change as rows are added or removed, while the chapter ID in its path
remains the identity. The sources have different title grammars; their numeric
fallbacks are not interchangeable.

Guya uses the lexicographically first scanlation-group key consistently for
chapter metadata and page URLs. Its catalog occupies one page: later browse or
search pages are empty. Latest uses descending `last_updated` with title as a
tie-breaker; popular and search use alphabetical order. The API supplies neither
popularity nor publication status, so no ranking or status is inferred.
Catalog records are decoded individually: malformed records are logged and
skipped while valid siblings retain their ordering. Missing descriptive fields
are optional; a nonempty catalog with no usable records is an error.
Older stored Guya paths containing literal percent escapes may resolve differently
under the encoded-key format; refresh chapter identities before using those paths.

MangaDex excludes external-provider chapters and future publications. Feed
requests currently select English chapters, even though source metadata exposes
`Lang::All`. Other languages are not enabled by that metadata declaration. Feed
traversal requires consistent offsets, limits and totals, and rejects early empty
pages or repeated release IDs. A changed total fails the refresh immediately;
retrying is a new request. These checks do not provide an API snapshot guarantee
or detect every same-total replacement. Similarly, HTML entry validation cannot
detect a removed row that no longer matches any selector when other rows still
match. A successful result is not a universal proof of source completeness.

MangaDex titles prefer nonblank English (`en`), romanized Japanese (`ja-ro`),
then Japanese (`ja`). If none is available, the first nonblank title by sorted
language code is used. Descriptions remain English-only; covers and descriptive
metadata are optional. Unknown relationship kinds are ignored when the mapper
does not consume them, while required manga/chapter attributes and identities
remain validated. A malformed browsing record is logged and skipped if other
valid records remain; malformed detail or chapter data fails the request.

NHentai caches validated gallery metadata for 15 seconds with a four-entry
capacity. Detail and the synthetic chapter share that record. Its selected first
CDN server is validated before entering the single-entry six-hour cache; invalid
configuration returns an error without trying another server. Gallery page
numbers must be consecutive starting at 1 after sorting. This detects gaps and
duplicates, but cannot detect missing trailing pages without an independent
expected count.

WeebCentral uses the same known genre-label aliases in lists and details and
reads status by label rather than metadata position. Reader requests retain a
separate one-request-per-second budget. Relative assets use the final document
URL after redirects. Source identity links trust the configured origin and the
current response's final origin; asset CDN hosts are independent of that policy.
