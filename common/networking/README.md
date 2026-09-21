# Networking ownership and client policy

`lib.rs` exposes typed operations; raw ureq requests/responses and solver wire
models stay internal. `client.rs` owns direct HTTP, `image.rs` interprets image
responses, `ratelimit.rs` owns pacing, `operation.rs` bounds each fetch, and
`backoff.rs` parses Retry-After values. `flaresolverr/` owns browser routing,
configuration, RPCs, cookies, session lifecycle, solve coordination and
browser-rendered JSON decoding.

## Construction and current callers

| Source | Construction and routing | Configured requests/second |
|---|---|---|
| Guya | `build_rate_limited_ureq_agent`, direct | 10 |
| MangaDex | Direct API/image client; independent direct At-Home client | 5; At-Home 0.6 |
| Manhwa18cc | Direct client through shared Madara helpers | 10 |
| NHentai | `build_rate_limited_flaresolverr_client_for_extension` for HTML/API; separate direct CDN client | 10 each |
| WeebCentral | Direct client; independent direct chapter-pages client | 10; pages 1 |

Direct clients offer `fetch_document`, `fetch_text`, `fetch_json` and
`fetch_bytes`. The JSON helper uses ureq's existing body limits.
Browser-returned API JSON may be wrapped in
HTML: NHentai explicitly uses `parse_browser_json`, which accepts raw JSON or
JSON inside the first `<pre>` element. Ordinary direct JSON does not opt into
HTML parsing.

## Documents, final URLs and parser boundaries

`FetchedDocument { final_url, body }` retains the actual response URL after
redirects. Both direct and solver clients offer `fetch_document`; the solver
client also offers `post_form_document` and `post_empty_document`. Existing
text-only methods delegate to the same request implementation and return its
body. Direct responses obtain the URL from ureq; solver responses retain the
reported solution URL. Parsers resolving relative links should use `final_url`,
not reconstruct a base from the original request.

Source parsing stays outside networking. The small helpers in
`extension-utils` distinguish image/cover URLs from source identity links:
assets may use independent CDN hosts, while identity links trust only the
configured source origin and the current response's final origin. This does
not establish a global list of trusted redirect hosts. Stored source paths
receive structural validation before requests, without replacing each source's
route or chapter conventions.

The local redirect regression covers GET and POST-to-GET 303 redirects for
form and empty POST bodies. The lockfile uses ureq 3.2.0, which fixes stale body
framing headers on redirected GET requests. Redirect following remains enabled,
and the final response URL and body are retained.

## Solver configuration and ownership

`FlareClient::new(FlareClientConfig)` is the single solver-client constructor.
`FlareClientConfig::from_env` is the sole environment adapter. The extension
helper supplies the managed name `tanoshi-{extension_name}`. An explicit,
nonblank `FLARESOLVERR_SESSION` overrides it when `FLARESOLVERR_URL` is present.
Construction does no network I/O; managed sessions and solving remain lazy.
Default explicit configuration has no solver or limiter. Missing solver
configuration remains supported direct-only mode.

An explicitly configured solver endpoint must be an absolute HTTP(S) URL with
a host and no whitespace/control characters. Empty and non-UTF-8
`FLARESOLVERR_URL` values are invalid. Blank or unreadable
`FLARESOLVERR_SESSION` values do not supply an external override. Invalid
endpoint configuration is stored during construction; every
text, POST or image operation on that solver client logs and returns a descriptive
configuration error before making a request. Diagnostics do not echo the endpoint,
which may contain credentials. Custom endpoint paths are allowed.

Validation does not probe the service. A valid but unavailable solver does not
prevent construction or successful direct requests; connectivity errors arise
only when solving/proxying is needed. Direct-only clients are unaffected.

Managed sessions always use their configured extension name. If FlareSolverr
reports that a cached session disappeared (for example, after a restart), the
client discovers or recreates that named session and retries the solver request
once. Initialization/recovery failures are logged and returned as operation
errors; they never trigger a nameless temporary-session fallback. A creation
response must confirm the requested name. If creation fails, the client re-lists
once and reuses the exact requested name only if it now exists, covering creation
by another process or a lost response.
A late missing-session response cannot invalidate a newer recovered session,
even though both use the same server-side name. Session ownership is represented
explicitly as stateless, external or managed state.

Explicit `FLARESOLVERR_SESSION` values are passed through without checking
existence or issuing session creation/recovery commands. FlareSolverr 3.5.2
automatically creates a missing session under the supplied ID when handling a
browser request. This behavior is allowed, including after a server restart;
it does not create a nameless temporary session. A recreated browser does not
retain the previous session's cookies or session-specific proxy configuration,
and a mistyped ID can create an unintended persistent session. Operators who
prepare sessions externally must restore that state themselves. Creation or
solve failures still return operation errors. See the server's
[session lookup implementation](https://github.com/FlareSolverr/FlareSolverr/blob/v3.5.2/src/sessions.py#L68-L74).

Requests on the same client or its clones share an in-flight challenge solve,
including its success or failure. They capture the attempt before direct I/O,
so a late challenge response can reuse a solve that has already finished. Later
operations may start a fresh solve; errors are not cached permanently. Solver
I/O does not hold the general client-state lock. Separately constructed clients
retain independent agents, cookies and pacing. All browser navigation (solving
and proxy GET/POST requests) is serialized by normalized solver endpoint and
session ID, including independently constructed clients sharing a managed or
external session. Waiting counts toward the operation deadline; different
sessions and stateless requests can proceed concurrently. This coordination is
local to the linked networking library: other plugins, processes, applications,
or different endpoint aliases for the same solver are outside its lock.

The non-extension solver builder and solver POST/image methods are retained for
Madara's deliberately preserved AJAX paths and the excluded HariManga restoration
candidate. These candidates still require an extension update; retaining an API
does not establish that the old wrappers compile or that their websites work.

## Pacing and shared state

Client clones share state and pacing; separately constructed clients/extension
instances have independent budgets.
Direct and browser clients pace each explicit website request attempt, including
re-solving and proxy retries. Session list/create RPCs use the operation deadline
but do not spend the website rate allowance. Internal
HTTP redirects and browser subrequests are not separately paced here.

Configuration validation, attempt pacing, deadlines, HTTP 429 backoff, session
coordination, opt-in wrapper traversal, image format recognition and HTTP-200
challenge handling are implemented as described below. Networking has not been
split into optional dependency features, and these module boundaries do not
establish a compile-time improvement or binary-size reduction.

## Diagnostics and verification

`init_plugin_logging` initializes a logger in each linked plugin because the
host's logger does not automatically reach the plugin's `log` facade. Logging
defaults to `info`, follows `RUST_LOG`, and writes to stderr. For example,
`RUST_LOG=networking=debug,nhentai=debug` includes transport, parser and cache
diagnostics. Configuration errors and unrecovered request/session failures
return operation errors to the caller. Intermediate solve/retry failures also
produce logs, even when a later allowed attempt succeeds. Networking does not
implement a user-notification UI.

Run local checks without FlareSolverr; these include HTTP servers on local
sockets. Run the ignored live checks serially with a reachable solver:

```sh
cargo test -p networking --locked
curl --fail --silent --show-error --max-time 10 http://localhost:8191/
FLARESOLVERR_URL=http://localhost:8191/v1 cargo test -p networking --locked -- --ignored --test-threads=1
```

Add `--offline` when dependencies are already cached. Live checks depend on real
sites and the solver's ability to solve their current challenges; HTTP 200 from
the solver is not proof of a successful challenge solve. The
[contributor guide](../../docs/development.md#build-and-verify) explains test tiers,
host-network access and current CI behavior.

## Rate configuration

`None` explicitly selects unlimited requests. A supplied rate must be positive,
finite and convert to a nonzero interval that the platform's monotonic clock can
schedule. Invalid rates are retained as configuration errors: construction stays
infallible, while direct and solver client operations log and return an error
before making requests. Rates are never silently replaced or clamped. Current
source constants and separate endpoint budgets are unchanged.

The limiter also checks clock arithmetic when reserving each attempt and recovers
a poisoned scheduling mutex. Client clones continue to share their limiter.

## Operation budgets

Each public networking fetch has a 240-second total budget. Individual HTTP calls
(including reading their bodies and following redirects) and browser solves are
capped at 120 seconds or the remaining budget, whichever is shorter. Wrapper hops,
pacing, session RPCs/recovery and shared-solve waits use the same operation budget.
Long waits that cannot fit return errors. A separate safety cap allows at most
16 explicit website request attempts, preserving room for the existing bounded
wrapper and recovery paths. Session-management RPCs remain structurally bounded.

Lock waits protecting network work check the deadline in 10 ms intervals; the
general client-state lock is held only for short state access. Browser maxTimeout
is computed after pacing, immediately before its RPC. The library cannot cancel
server-side work after an HTTP timeout. Internal HTTP redirects and browser
subresources are not separately paced or counted as explicit attempts. This
budget covers a networking fetch, not an entire multi-page extension action.

## HTTP 429 backoff

An ordinary direct HTTP 429 response permits one retry per operation, shared
across retries and image wrapper hops. Both clients honor `Retry-After` seconds
or an HTTP date; missing/invalid values use five seconds. A wait that cannot fit
the remaining operation budget returns an error immediately instead of retrying
early. The retry retains request headers, referer and POST form data and is paced
like every other website attempt.

An ordinary 429, or a failed rate-limit retry, does not escalate to FlareSolverr
or switch the client to proxy-only routing. An actual detected Cloudflare
challenge may still use the existing solve/retry path. HTTP 429 from the solver
RPC itself, or an explicitly reported upstream 429, returns an error without an
additional browser request. FlareSolverr versions that synthesize upstream status
200 cannot reliably expose an ordinary upstream 429; this library cannot infer a
missing status or `Retry-After` value from arbitrary browser-rendered content.

## Image wrappers

Both clients' `fetch_bytes` methods reject HTML/XHTML instead of selecting an
arbitrary image from an unexpected page. A source known to return wrappers may
use `fetch_bytes_with_wrapper_policy` with `ImageWrapperPolicy::FirstImage`.
None of the five active sources opts in. NHentai retains its separate direct
CDN client; the solver-aware API remains available for the HariManga candidate.

Both request policies use the same traversal, allowing at most two wrapper hops.
The first `img[src]` must have a nonblank target; it resolves against the final
HTTP response URL after redirects. Targets must use HTTP(S). Fragment differences
do not bypass cycle detection, which tracks requested URLs and final wrapper
URLs. Source referers, pacing, operation budgets and the 50 MiB binary body limit
are preserved.

## Image response validation

The shared parser requires a recognized JPEG, PNG/APNG, GIF, WebP, BMP or AVIF
signature. It accepts supported image MIME types, absent Content-Type, and the
generic binary types application/octet-stream, binary/octet-stream and
application/binary. The signature remains required under generic types and
image headers, so JSON/error text cannot pass merely by claiming to be an image.
Other text/JSON/unsupported MIME types, empty bodies and unrecognized formats
produce operation errors. Accept advertises the supported formats explicitly.

AVIF recognition checks the bounded ftyp box for an avif/avis major or compatible
brand, excluding the minor-version field. These checks identify file formats;
they do not decode images or prove every image is complete. No runtime decoder
dependency is used. The live test decoder supports JPEG/PNG/GIF/WebP;
there are no AVIF decoding checks or fixtures.

Challenge detection runs before wrapper interpretation, including HTTP-200
responses and mislabeled text bodies. Direct clients return a challenge error.
The solver-aware client shares the existing solve coordination and retries the
image once after a successful solve; a repeated challenge is an error and never
becomes an HTML wrapper/logo download. Ordinary invalid image content does not
trigger a browser solve. No active source switches its image path to the solver.
