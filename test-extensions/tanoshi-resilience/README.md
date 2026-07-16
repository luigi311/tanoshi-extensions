# Tanoshi resilience test extension

This standalone dynamic extension is a host-side test fixture. It is kept
outside the production extension workspace so it is not included in the
published source catalog.

Build it with:

```sh
cargo build --manifest-path test-extensions/tanoshi-resilience/Cargo.toml --release
```

Copy the resulting `libtanoshi_test_extension.so` (or the platform equivalent)
to a plugin directory as `tanoshi-test.so`, then load it with
`ExtensionManager::load("tanoshi-test")`.

The fixture starts in `normal` mode. Call `set_preferences` with text inputs
named `test_mode` and, for `block`, `test_release_file` to select a behavior:

| Mode | Behavior |
| --- | --- |
| `normal` | Return deterministic, network-free values. |
| `sleep` | Sleep for 100 ms, then return successfully. |
| `block` | Wait for the release-marker file to appear, with a 30 s safety deadline. |
| `panic_read` | Panic from a read operation. |
| `error` | Return a deterministic read-operation error. |
| `panic_preferences` | Panic from `set_preferences`. |
| `error_preferences` | Return a deterministic `set_preferences` error. |

The release-marker file is considered released when it exists. The fixture's
bounded safety deadline prevents an accidentally abandoned test call from
blocking forever.
