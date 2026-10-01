# Contributing

## Commit messages

This project uses [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

```
<type>(<optional scope>): <subject>
```

The subject is imperative, lowercase, and has no trailing period. Keep the
subject under 72 characters; put detail in the body, separated by a blank
line, wrapping at 72 columns.

### Types

| Type | Use for |
| --- | --- |
| `feat` | A new user-visible capability |
| `fix` | A bug fix |
| `docs` | Documentation only |
| `style` | Formatting with no behavioural change |
| `refactor` | No behaviour change, no feature |
| `perf` | A performance change |
| `test` | Adding or fixing tests |
| `build` | Build system, dependencies, packaging |
| `ci` | CI configuration |
| `chore` | Anything else |

### Scopes

Use the module or area: `auth`, `sync`, `store`, `api`, `ui`, `tray`,
`notify`, `packaging`.

### Examples

```
feat(sync): fall back to a full sync on 410 Gone
fix(api): follow nextPageToken until exhausted
docs(readme): document the missing device-code flow
build(deps): bump gtk4 to 0.11.5
```

### Breaking changes

A `!` before the colon plus a `BREAKING CHANGE:` footer:

```
feat(api)!: rename Task.due to Task.due_date

BREAKING CHANGE: the DTO field name changed to match the API's
`due` semantics more precisely.
```

Since this is an application rather than a library, breaking changes mostly
matter to the on-disk schema and config file format. When you change either,
say so in the commit body and bump the version accordingly.

## Branches

`main` is the default branch and is always expected to build. Work in
descriptive branches off `main` and open a pull request.

Branch names follow the type of work: `feat-quick-add`, `fix-etag-retry`,
`docs-api-limits`.

## Before opening a pull request

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same three checks, plus a release build. All of them must pass.

## Code style

- Follow the surrounding code. `cargo fmt` is authoritative for formatting.
- Prefer explicit types at module boundaries. Clippy warnings are errors.
- Document the *why* behind non-obvious decisions, especially anything
  working around a Google Tasks API limitation. Those are the parts a future
  reader will otherwise assume are mistakes.
- Do not add comments that merely restate what the code does.

## Architecture

Read [docs/architecture.md](docs/architecture.md) first. The key constraint is
that the `ui/` layer never performs network I/O; it reads from `store/` and
issues intents to `sync/`. Please keep it that way.

## Reporting bugs

Open an issue with:

- what you did
- what you expected
- what happened instead
- the output of `RUST_LOG=gtaskbar=debug gtaskbar`
- your distribution, GTK4 version and libadwaita version
