# CLAUDE.md — `flint`

Flint is an in-house GPUI component library + semantic theme layer, built on raw
GPUI with **zero application coupling** so any GPUI app can share it verbatim. It
was extracted from the Nyx SFTP client and is consumed by Nyx and RED. Treat
these as hard constraints when editing here.

## The one rule that matters most

> **Flint must never depend on anything application-specific.** No consumer crate,
> no domain types, no app state. It depends on `gpui` only (the gallery example
> may use `gpui_platform` as a dev-dependency — fine; it's not an app dep).

If you reach for a domain type here, stop — the design is wrong. Map domain types
to generic component props *in the consuming app*, not in the component.

## The GPUI-rev contract

GPUI is pinned to an exact Zed rev in `Cargo.toml`. That rev is a **shared
contract** with every consumer: Cargo unifies two git deps only at the same
URL+rev, so a consumer must resolve gpui to this exact rev or get a second,
incompatible gpui copy (its `App`/`Element`/`Window` types won't interoperate).
Flint re-exports it (`pub use gpui;`) so consumers use `flint::gpui` and never
declare their own. Bumping the rev is a coordinated, multi-repo event.

## Rules

- **No domain types in signatures.** A row renderer takes `impl IntoElement` or a
  closure — never a concrete domain struct.
- **Tokens are semantic and generic** (`bg_panel`, `accent`) — never
  app-specific names. App-specific styling lives in the app.
- **Public API via the `prelude`**: `use flint::prelude::*;`.
- **Gallery-first.** Every public component gets a gallery entry
  (`examples/gallery.rs`). Iterate there before wiring into an app:
  `cargo run -p flint --example gallery`. Doc comments only when they add
  non-obvious info.
- **GPL-3.0-or-later license header** (`// SPDX-License-Identifier:
  GPL-3.0-or-later`) on every source file.
- **No external widget crate.** Build on GPUI primitives (`div`, the styling
  API). That includes `TextInput` — built in-house.

## Layout

- `theme.rs` — `Theme` token struct, `Global` impl, `ActiveTheme` accessor.
- `tokens.rs` — concrete One Dark / GitHub Dark / Ayu tables.
- `styled_ext.rs` — `StyledExt`: theme-aware style recipes (the "@apply" layer).
- `components/` — one file per component, variant API (`Button` is the reference).
- `examples/gallery.rs` — the storybook.

The canonical extraction history and roadmap live in the Nyx repo at
`docs/plans/plan-02-nyx-ui-flint.md`.
