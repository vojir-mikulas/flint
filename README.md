# Flint

An in-house **GPUI** component library and semantic theme layer, in pure Rust.
Built on raw [GPUI](https://github.com/zed-industries/zed) with **zero
application coupling** so any GPUI app can share it verbatim — extracted from the
[Nyx](https://github.com/vojir-mikulas/nyx) SFTP client and consumed by Nyx and
RED.

No web stack, no external widget crates — every component is built on GPUI
primitives, including the in-house `TextInput`.

## What's in the box

Button · IconButton · TextInput · Select · Segmented · Toggle · Tabs · Badge ·
Toast · Tooltip · Modal · ContextMenu · ProgressBar · a virtualized `Table`, plus
a semantic `Theme` token system (One Dark / GitHub Dark / Ayu) and a `StyledExt`
"@apply"-style recipe layer.

## Use it

Flint pins GPUI to an exact Zed git rev and **re-exports it** as `flint::gpui`.
Consumers must use that re-export (or pin the identical rev) — Cargo unifies two
git deps only at the same URL+rev, so a mismatched rev produces a second,
incompatible copy of GPUI whose types won't interoperate.

```toml
[dependencies]
flint = { git = "https://github.com/vojir-mikulas/flint" }
# use flint::gpui — do not declare a separate gpui dependency
```

```rust
use flint::{gpui, prelude::*, theme::Theme};

cx.set_global(Theme::one_dark());                 // install the theme global once
// in a view's render:
div().panel(cx).child(Button::new("run").on_click(/* … */))
```

## Develop

```sh
cargo run --example gallery   # the component gallery ("storybook")
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

Every public component has a gallery entry — iterate there before wiring into an
app. See [`CLAUDE.md`](CLAUDE.md) for the architecture rules.

## License

GPL-3.0-or-later. Flint links GPUI, whose dependency tree includes
GPL-3.0-or-later crates, so the combined work is GPL — see [`NOTICE`](NOTICE).
