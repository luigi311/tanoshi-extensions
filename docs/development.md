# Development guide

## Workspace and ownership

Five extensions are built: Guya, MangaDex, Manhwa18cc, NHentai and WeebCentral.
The workspace contains these five crates and three shared libraries:

| Location | Responsibility |
|----------|----------------|
| `extensions/guya` | Guya API requests, source DTOs and chapter/group mapping |
| `extensions/mangadex` | MangaDex API requests, endpoint DTOs, mapping and filter/query catalog |
| `extensions/manhwa18cc` | Manhwa18cc browsing and reader parsing; uses shared Madara metadata/chapter helpers |
| `extensions/nhentai` | Source-local API, parsing, query and bounded gallery/CDN caches |
| `extensions/weebcentral` | Source-local requests, pure parsers and listing query serialization |
| `common/networking` | Direct/solver clients, pacing, deadlines, sessions, response and image handling |
| `common/extension-utils` | Plugin registration, small URL/text/preference helpers and dev-only image assertions |
| `common/madara` | Shared HTML/AJAX helpers for Manhwa18cc and deliberately retained restoration candidates |

Guya owns its implementation; there is no separate `guyalib` crate. Removed
engines are historical code, not workspace dependencies. Only
`extensions/dead/365manga` and `extensions/dead/tritiniascans` remain as excluded
restoration references. See [Madara ownership](../common/madara/README.md) and the
[source catalog](sources.md).

## Build and verify

Run commands from the repository root. Rust is pinned to **1.93.1** in
[`rust-toolchain.toml`](../rust-toolchain.toml); `tanoshi-lib` is pinned to tag
**v0.38.0** in the root [`Cargo.toml`](../Cargo.toml). Dependencies and the edition
are primarily inherited from the workspace, with resolutions in `Cargo.lock`.
Toolchain changes must be coordinated in order: tanoshi-builder, Tanoshi, then
this repository.

```sh
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo test --workspace --locked
cargo build --workspace --release --locked
```

`--workspace` explicitly selects every active crate; `--locked` prevents an
implicit lockfile update. Add `--offline` when the required dependencies are
already cached. Local release libraries are written to `target/release`.

Default tests exercise local logic and local HTTP servers without contacting
source websites or FlareSolverr. They may require permission to bind local
sockets in a sandbox. External source checks carry
`#[ignore = "live source check"]` and are run separately, serially:

```sh
curl --fail --silent --show-error --max-time 10 http://localhost:8191/
FLARESOLVERR_URL=http://localhost:8191/v1 cargo test --workspace --locked -- --ignored --test-threads=1

# One source, local checks followed by live checks:
cargo test -p nhentai --locked
FLARESOLVERR_URL=http://localhost:8191/v1 cargo test -p nhentai --locked -- --ignored --test-threads=1
```

`--ignored` runs only ignored checks; it does not also run the default suite.
The readiness URL above checks the service root, while `FLARESOLVERR_URL` points
to its RPC endpoint. If no service is running, a local instance can be started
with:

```sh
docker run -d --name=flaresolverr -p 8191:8191 ghcr.io/flaresolverr/flaresolverr:latest
```

Wait for readiness before running live checks. Use an existing host service
when available. If a sandbox cannot reach host localhost, run the preflight and
tests in the same host network context; do not start a duplicate container just
because the sandbox cannot see it. Set `FLARESOLVERR_URL` on each live command
or export it in the shell running Cargo.

Live failures can indicate a site outage, changed markup, expired image URL or
unsolved challenge, as well as an extension defect. Inspect the returned error
and logs. The original ignored MangaDex `test_large_image` still uses an external
CDN URL that can expire. Sampled live image checks decode JPEG/PNG/GIF/WebP;
runtime AVIF recognition does not imply an AVIF decoding test exists.

## Solver configuration and diagnostics

NHentai uses `FlareClient` for HTML/API requests and a separate direct client
for CDN images. The other active sources use direct clients. Setting a solver
endpoint does not automatically route every extension through it.

| Setting | Behavior |
|---------|----------|
| `FLARESOLVERR_URL` | Optional absolute HTTP(S) RPC URL, commonly `http://localhost:8191/v1`. Unset means direct-only operation; an explicitly empty or invalid endpoint is an error. |
| `FLARESOLVERR_SESSION` | A nonblank externally managed session ID overrides the extension-managed session when an endpoint is configured. It is ignored without an endpoint. |
| `RUST_LOG` | Plugin logging filter; defaults to `info` and writes to stderr. For example, `RUST_LOG=networking=debug,nhentai=debug` enables transport and NHentai diagnostics. |

Construction performs no network I/O. Invalid solver configuration is reported
by operations; a valid but unavailable solver only causes a connectivity error
when solving is needed. Successful direct requests can still work.

NHentai lazily discovers or creates its managed `tanoshi-nhentai` session. If
FlareSolverr restarts and loses it, the client attempts recovery under that same
name and retries the solver request once. A failed recovery returns an error;
there is no nameless temporary-session fallback. Explicit external session IDs
are passed through without client-side existence checks or creation commands.
FlareSolverr may recreate a missing session under the same supplied ID; previous
cookies and session-specific proxy configuration are not restored. See the
[networking policy](../common/networking/README.md#solver-configuration-and-ownership)
for details.

Each networking fetch has a 240-second total budget; individual HTTP/solver
requests are capped at 120 seconds or the remaining budget. This is a per-fetch
budget, not a timeout for an entire multi-page extension action. See the
[networking policy](../common/networking/README.md) for rates, retry boundaries,
cookies, image validation and wrapper opt-in behavior.

## Current CI and artifact publication

The checked-in workflows run as follows:

| Workflow | Trigger | Checks and outputs |
|----------|---------|--------------------|
| [CI](../.github/workflows/ci.yml) | Push and manual dispatch; Markdown-only pushes and `.gitignore` changes are excluded by its path filter | `cargo test`, then serial ignored live checks with FlareSolverr; release builds for three platforms; publication from the default branch |
| [Daily source checks](../.github/workflows/daily-source-checks.yml) | Daily at 13:17 UTC | `cargo test --workspace --locked`, then `cargo test --workspace --locked -- --test-threads=1 --ignored` with `FLARESOLVERR_URL` set |

Both workflows treat live-test failures as failures. Both currently start the
`latest` solver image, wait a fixed five seconds and stop it after the test
command. There is no always-run cleanup step. CI has no `pull_request` trigger,
and its publication job depends on **build only**, not the test job. Main CI
test/build commands do not currently use `--locked`; the local commands above
are stricter.

Builds produce Linux x86_64 and aarch64 `.so` libraries and Windows x86_64
`.dll` libraries. The Linux x86_64 job downloads `tanoshi-cli` from Tanoshi's
latest release to generate `index.json`; its version is not separately pinned
in the workflow. Platform artifacts are merged into a checkout of the branch
named by CI's `RUST_TOOLCHAIN` value, currently `1.93.1`, committed and pushed.
The pin is duplicated between CI and the toolchain file. The push action does
not explicitly request a force push, and the commit step has no explicit
no-change guard.

The published layout has `index.json` at the branch root and libraries under
`x86_64-unknown-linux-gnu/`, `aarch64-unknown-linux-gnu/` and
`x86_64-pc-windows-msvc/`. The index is generated from plugin metadata; do not
hand-maintain a second source of extension IDs or versions.

## Changing, adding or retiring a source

Keep source-specific selectors, DTOs, query syntax and chapter rules beside
their source. Reuse the small shared helpers where their semantics match.
Prefer existing checks for routine changes; add focused regression coverage
when a changed functional contract needs it. Mark real website checks as
ignored live tests and sample images rather than downloading whole chapters.

Extension package versions are how the host detects updates. Review the
affected extension versions whenever runtime behavior changes, including
behavior inherited from a shared library. A networking or extension-utils
change can affect all five extensions; a Madara change currently affects
Manhwa18cc among built sources. Follow the actual reverse dependencies and
affected call paths. Shared crates' `0.0.0` versions do not replace extension
version bumps. Pure documentation changes or mechanical moves need no bump;
record that reason. Multiple pieces in the same unreleased update may share
one extension bump. This review is currently manual.

To add a source, create its `cdylib` crate under `extensions/`, implement the
pinned host interface, register through `extension_utils::export_extension!`,
and add a catalog entry with a new, unused permanent ID. Review source metadata,
path identity, transport policy, dependencies and existing target build layout.
Verify local checks and the relevant live operations before marking it healthy.
Sources without preferences can use the trait's empty defaults.

To retire a source, update the lifecycle/health catalog separately, preserve a
Git revision from which the implementation can be recovered, and remove it
from active workspace membership. Retain excluded code only for an intentional
restoration candidate. Never reuse a retired ID or silently transfer saved
paths to an unrelated backend. A replacement implementation for a changed
source structure requires a new ID under this repository's identity policy.
Restoring either retained candidate requires an explicit compatibility review;
keeping its files does not make it buildable.
