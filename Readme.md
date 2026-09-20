# Tanoshi Extensions
This repository host extenstions for tanoshi

## Source list
| ID  | Name                | Note                                       | Status  |
|-----|---------------------|--------------------------------------------|---------|
| 2   | Mangadex            |                                            | Active  |
| 3   | Mangasee            | Site closed                                | Removed |
| 4   | Manga4Life          | Site closed                                | Removed |
| 5   | Catmanga            | Site closed                                | Removed |
| 6   | NHentai             |                                            | Broken  |
| 7   | Guya.moe            |                                            | Active  |
| 8   | Manhwa18.cc         |                                            | Active  |
| 9   | Nhentai             | Use nhentai api with different path than 6 | Removed |
| 10  | Mangakakalot        |                                            | Active  |
| 11  | Manganato           |                                            | Active  |
| 12  | ManhuaFast          |                                            | Broken  |
| 13  | MangaTX             | Site structure changed, use 27             | Removed |
| 14  | Leviatan Scans      | Site structure changed, use 26             | Removed |
| 15  | Reaper Scans        |                                            | Broken  |
| 16  | 1st Kiss Manhua     |                                            | Active  |
| 17  | 365Manga            |                                            | Broken  |
| 18  | IsekaiScan.com      | Site closed                                | Removed |
| 19  | MMScans             | Site closed                                | Removed |
| 20  | TritiniaScans       |                                            | Active  |
| 21  | AlphaScans          | Site closed                                | Removed |
| 22  | AsuraScans          | Site structure changed, use 24             | Removed |
| 23  | Isekaiscanmanga.com |                                            | Active  |
| 24  | Asurascans          | Site structure changed, use 25             | Removed |
| 25  | AsuraScans          |                                            | Active  |
| 26  | LeviatanScans       |                                            | Active  |
| 27  | MangaTX             | Site closed                                | Removed |
| 28  | WeebCentral         |                                            | Active  |
## Chapter identity, ordering and fallbacks

Chapter paths identify releases. Chapter numbers are display/sort metadata and can
repeat; distinct paths must not be merged just because their numbers match. The
host also applies its own ordering to stored chapters.

| Source | Chapter number and fallback | Upload time and fallback | Extension output order |
|--------|-----------------------------|--------------------------|------------------------|
| MangaDex | Parse the API chapter label as a number; missing or unparseable labels use `0`. | Required API `publishAt`, converted to Unix seconds; malformed or missing timestamps fail decoding. | Feed creation time ascending. Different release IDs with the same number remain separate. |
| Guya | Parse the chapter-map key as a number; unparseable keys use `0`. The original key remains in the path. | The selected group's release timestamp is cast to integer seconds; a missing timestamp uses `0`. | Number ascending, then path for equal numbers. |
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

MangaDex excludes external-provider chapters and future publications. Feed
traversal requires consistent offsets, limits and totals, and rejects early empty
pages or repeated release IDs. A changed total fails the refresh immediately;
retrying is a new request. These checks do not provide an API snapshot guarantee
or detect every same-total replacement. Similarly, HTML entry validation cannot
detect a removed row that no longer matches any selector when other rows still
match. A successful result is not a universal proof of source completeness.

## Diclaimer
The developer of this application does not host any content and does not have affiliation with any content provider.
