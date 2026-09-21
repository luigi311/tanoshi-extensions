# Retained restoration candidates

These two extensions are kept as reference for possible restoration. They are
excluded from the workspace and are **not supported or expected to compile**.

| Directory | Reserved source ID | Configured site |
|-----------|--------------------|-----------------|
| `tritiniascans` | 20 | https://tritinia.org |
| `365manga` | 17 | https://www.harimanga.co.uk |

The `365manga` package still identifies itself as 365Manga, despite already
targeting HariManga. Resolve that history and stored-path compatibility before
restoring or renaming the extension.

Both use `madara` and `networking`. Their old dependency paths, plugin APIs and
request/parser calls need review against the current workspace. Keeping these
files does not make the manifests buildable. Their source and manifest contents
were preserved unchanged during the September 2026 retirement cleanup.

Keep these prospective Madara consumers in mind when reviewing shared-engine
ownership; restoring them would change the current single-active-consumer count.
Restoration remains a separate decision and implementation task.

See the [source catalog and recovery instructions](../../docs/sources.md) for the
other retired sources. Their IDs remain reserved even though their code has been
removed from the current tree.
