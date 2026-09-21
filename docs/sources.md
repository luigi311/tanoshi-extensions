# Source catalog

Source IDs are permanent and must never be reused. Lifecycle describes whether
code ships in this workspace; health describes the last recorded verification,
not a guarantee of current website availability. Health review: **2026-09-20**.

| ID | Source | Lifecycle | Health / verification | History or disposition |
|----|--------|-----------|-----------------------|------------------------|
| 2 | MangaDex | Built | Live checks passed | English hosted-chapter feed |
| 3 | MangaSee | Removed | Original service superseded | Successor: WeebCentral (28) |
| 4 | Manga4Life / MangaLife | Removed | Original service superseded | Successor: WeebCentral (28) |
| 5 | Catmanga | Removed | Closed per historical catalog | ID remains reserved |
| 6 | NHentai | Built | Live checks passed | Solver fallback verified; retained source ID 6 |
| 7 | Guya | Built | Live checks passed | No inferred publication status or popularity ranking |
| 8 | Manhwa18cc | Built | Live checks passed | Active Madara consumer |
| 9 | Nhentai (old API) | Removed | Obsolete implementation | Different path scheme from ID 6 |
| 10 | Mangakakalot | Removed | Current reader unverified | Retired implementation discarded; timeout is not proof of closure |
| 11 | Manganato | Removed | Current reader unverified | Retired implementation discarded |
| 12 | ManhuaFast | Removed | Current reader unverified | Configured site returned an origin SSL error during audit |
| 13 | MangaTX (old) | Removed | Obsolete implementation | Historical successor ID 27, also retired |
| 14 | Leviatan Scans | Removed | Current reader unverified | Archived LSComic implementation still exports 14; see ID mismatch below |
| 15 | ReaperScans | Removed | Original service closed | May 2025 closure; similarly named sites are not verified successors |
| 16 | 1st Kiss Manhua | Removed | Current reader unverified | Retired implementation discarded |
| 17 | 365Manga / HariManga | Retained reference | Site and chapter markup verified; plugin unverified | Excluded restoration candidate in `extensions/dead/365manga` |
| 18 | IsekaiScan.com | Removed | Closed per historical catalog | Distinct from ID 23 |
| 19 | MMScans | Removed | Current reader unverified | Retired implementation discarded |
| 20 | TritiniaScans | Retained reference | Site and chapter markup verified; plugin unverified | Excluded restoration candidate in `extensions/dead/tritiniascans` |
| 21 | AlphaScans | Removed | Closed per historical catalog | ID remains reserved |
| 22 | AsuraScans (old) | Removed | Obsolete implementation | Historical successor ID 24 |
| 23 | IsekaiScanManga | Removed | Current reader unverified | Distinct from ID 18; retired implementation discarded |
| 24 | AsuraScans (intermediate) | Removed | Obsolete implementation | Historical successor ID 25 |
| 25 | AsuraScans | Removed | Site exists with a different backend | Retired WP Manga Stream adapter discarded; restoration needs replacement work |
| 26 | LeviatanScans (catalog entry) | Reserved historical entry | No matching implementation in retirement snapshot | Do not reassign; see ID mismatch below |
| 27 | MangaTX | Removed | No verified current reader | Configured domain redirected away from a verified manga reader |
| 28 | WeebCentral | Built | Live checks passed | Replaces MangaSee/MangaLife |

Only TritiniaScans and 365Manga/HariManga are retained as restoration candidates.
Both require modernization before building; their old
manifests and source were preserved unchanged. See the
[retained-source notes](../extensions/dead/README.md). A removed implementation does
not imply that every similarly named website is closed.

The website audit used the current [Asura catalog](https://asurascans.com/),
[Tritinia](https://tritinia.org/), and [HariManga](https://www.harimanga.co.uk/home).
[WeebCentral's FAQ](https://weebcentral.com/faq) confirms the MangaSee/MangaLife
rebrand. [Kakao's May 2025 statement](https://kakaoent.com/pr/detail/258) records
the original ReaperScans closure. A timeout, TLS failure or similarly named mirror
does not establish permanent closure or continuity of ownership.

### Historical source recovery

The complete pre-cleanup tree is preserved at immutable commit
[`6aa1543d72d300da4bde5a374488399fdd478ccd`](https://github.com/luigi311/tanoshi-extensions/tree/6aa1543d72d300da4bde5a374488399fdd478ccd).
It contains all 14 retired extensions and four retired engines as they existed
before cleanup. The removed paths at that revision are:

| Tree | Removed directories |
|------|---------------------|
| `extensions/dead/` | `asurascans`, `firstkissmanhua`, `isekaiscanmanga`, `leviatanscans`, `mangakakalot`, `mangalife`, `manganato`, `mangasee`, `mangatx`, `manhuafast`, `mmscans`, `reaperscans` |
| `common/dead/` | `mangakakalot`, `nepnep`, `wpmangareader`, `wpmangastream` |

To inspect that code without changing the working tree:

```sh
git show 6aa1543d72d300da4bde5a374488399fdd478ccd:extensions/dead/asurascans/src/lib.rs
```

To recover the full reference tree into a separate directory:

```sh
mkdir -p /tmp/tanoshi-retired-reference
git archive 6aa1543d72d300da4bde5a374488399fdd478ccd extensions/dead common/dead | tar -x -C /tmp/tanoshi-retired-reference
```

This snapshot is historical reference, **not a buildable archive**: relative
dependency paths, plugin APIs and source protocols may be obsolete. Before the
retirement moves, these packages lived at `extensions/<name>` and
`common/<name>`. Use `git log --follow -- <archived-file-path>` at the reference
revision to trace those earlier locations. Sources already absent from the
snapshot remain listed above to preserve their reserved IDs.

### Leviatan ID 14/26 discrepancy

Commit
[`ed3f4d8946310e0076eabafde5fe15682e13539b`](https://github.com/luigi311/tanoshi-extensions/commit/ed3f4d8946310e0076eabafde5fe15682e13539b)
(2023-09-10) changed the site URL to `lscomic.com` and added catalog ID 26 as the
replacement for 14, but left the implementation's `const ID: i64 = 14` unchanged.
Commit `628a7e00b9e37c3ea00dae75a53850ee2f289e7c` later moved that implementation
into `extensions/dead/leviatanscans` without changing its ID. Both 14 and 26 stay
reserved; this cleanup neither renumbers historical code nor invents an ID-26
implementation.
