# UI Theme Guideline — "Ember Dark"

A warm, flat, monospace dark theme for `yeelight-gui`, built around an orange
accent on a warm-brown base.

This is a **design spec only**. No code is changed by this document; the
"Applying it" section is the implementation plan for later.

---

## 1. The system

Ember's identity is not one color — it's a *system*:

- A single **custom `Theme::custom` palette** (6 roles), not iced's built-ins.
- **Flat surfaces**: no borders, no shadows. Elevation comes *only* from
  `lighten()`/`darken()` on the base background.
- **Tight, consistent tokens**: 4px radius, 12px text, 9px padding everywhere.
- **Monospace** everything (Hack).
- Text-on-accent uses the **background color**, not white.

---

## 2. Ember Dark palette (orange)

An orange accent over a warm-brown base. Legible semantic colors are kept
distinct (see the note below) so "connected" never looks like "error".

| Role         | Hex        | RGB             | Use                                            |
|--------------|------------|-----------------|------------------------------------------------|
| `background` | `#26201a`  | 38, 32, 26      | app + panel base                               |
| `text`       | `#f2e0cf`  | 242, 224, 207   | primary text                                   |
| `text_muted` | `#b2a08f`  | 178, 160, 143   | subtitles, offline, placeholders               |
| `primary`    | `#fe8019`  | 254, 128, 25    | **the accent** — active chips, primary buttons, links |
| `success`    | `#a9b665`  | 169, 182, 101   | online / streaming / received                  |
| `danger`     | `#ea6962`  | 234, 105, 98    | errors                                         |
| `warning`    | `#e9b143`  | 233, 177, 67    | warnings                                       |

> **Monochrome vs. legible.** A fully-monochrome look — success/danger/warning
> all drawn from the accent family — is cohesive but trades usability: a
> bulb-control UI needs "online" (green) to read differently from "error" (red)
> at a glance. Ember keeps the orange *accent* as the identity but keeps
> semantic states distinguishable. If you want the fully-monochrome feel, set
> success `#f0a35e`, danger `#e8735a`, warning `#f2b56a` instead.

### Surface elevation (derived, not authored)

Never author panel colors. Derive them from `background`.

| Step             | Formula                    | Hex        | Use                          |
|------------------|----------------------------|------------|------------------------------|
| base             | `background`               | `#26201a`  | window, deepest panels       |
| raised           | `lighten(bg, 0.03)`        | `#2e2822`  | cards, secondary buttons, pick-lists |
| hover            | `lighten(bg, 0.12)`        | `#453f39`  | hovered rows/controls        |
| border / strong  | `lighten(bg, 0.15)`        | `#4c4640`  | 1px separators, section outlines |

`lighten`/`darken` clamp each channel after adding/subtracting a flat amount:

```rust
fn lighten(c: Color, a: f32) -> Color {
    Color { r: (c.r + a).min(1.0), g: (c.g + a).min(1.0), b: (c.b + a).min(1.0), a: c.a }
}
```

---

## 3. Design tokens

The token set (only the font is optional).

| Token           | Value           |
|-----------------|-----------------|
| corner radius   | `4.0`           |
| font size       | `12.0`          |
| padding         | `9.0`           |
| icon size       | `12.0`          |
| font family     | `Hack` (monospace) — *optional, see §6* |

Rule: **flat.** Borders `width: 0` (except the one 1px `border/strong`
separator), `Shadow::default()`, radius `4.0`. Depth is lightness, not shadow.

---

## 4. Component styling rules

**Primary button** — the accent call-to-action:
- Active: bg `primary`, text `background` (dark text on orange).
- Hover: bg `lighten(primary, 0.15)`, text `lighten(background, 0.1)`.
- Pressed: bg `lighten(primary, 0.03)`.
- Disabled: bg `lighten(primary, 0.05).scale_alpha(0.2)`, text `background.scale_alpha(0.5)`.

**Secondary button** — neutral surface:
- Active: bg `raised` (`lighten(bg, 0.03)`), text `text`.
- Hover: bg `hover` (`lighten(bg, 0.12–0.15)`).

**Pick list**:
- Active: bg `raised`, text `text`, placeholder `text_muted`.
- Hover: bg `lighten(bg, 0.12)`.

**Container / section box**: fill `raised`; if it needs an outline, 1px
`border/strong`, radius `4.0`.

**Active chip / tab**: bg `primary`, text `background` (not white — the
text-on-accent rule from §1).

---

## 5. Applying it to `yeelight-gui` (implementation plan — not done yet)

Today the GUI uses iced's **built-in** themes via a pick-list, plus ~8
hardcoded `Color::from_rgb(...)` accents scattered across views. The plan:

1. **New module `src/theme.rs`** (or an `appearance/` dir):
   - `ember_dark() -> iced::Theme` via `Theme::custom` with the §2 palette.
   - `lighten`/`darken` helpers.
   - Semantic accessors: `text_muted()`, `success()`, `danger()`, `warning()`,
     `surface_raised(theme)`, `surface_border(theme)`.
   - Style fns: `primary_button`, `secondary_button`, `pick_list`, `section`.
   - Token consts: `RADIUS = 4.0`, `PADDING = 9.0`, `FONT_SIZE = 12.0`.

2. **`app.rs`** — make Ember the default: `theme()` returns `ember_dark()`;
   `ThemePref::System` resolves to it. *Decision:* keep the existing theme
   pick-list (prepend Ember) so users can still choose built-ins, **or** drop
   the picker for a single-theme model. Recommend keeping it — near-zero cost,
   Ember just becomes the default entry.

3. **Replace the scattered hardcoded colors** with the semantic accessors:

   | File | Current | → Token |
   |------|---------|---------|
   | `view/rail.rs:18` | online `rgb(0.2,0.83,0.6)` | `success()` |
   | `view/rail.rs:20` | offline `rgb(0.42,0.45,0.5)` | `text_muted()` |
   | `view/detail/music.rs:16` | streaming `rgb(0.3,0.8,0.5)` | `success()` |
   | `view/detail/music.rs:19` | sub `rgb(0.55,0.58,0.63)` | `text_muted()` |
   | `view/detail/mod.rs:157` | subtitle `rgb(0.55,0.58,0.63)` | `text_muted()` |
   | `view/logging.rs:44` | sent `rgb(0.45,0.7,1.0)` | `primary` |
   | `view/logging.rs:45` | received `rgb(0.4,0.85,0.55)` | `success()` |
   | `view/mod.rs:41` | error `rgb(0.9,0.3,0.3)` | `danger()` |
   | `view/components.rs:27` | chip-active text `WHITE` | `palette.background` |
   | `view/detail/mod.rs:256` | section border `background.strong` | `surface_border()` |

4. **Tokens**: swap ad-hoc radii (6.0, 8.0, 14.0) toward `4.0`/`14.0`-for-pills
   as desired; apply `PADDING`/`FONT_SIZE` consts.

Scope: ~1 new module + edits to 6 view files. No new dependencies.

---

## 6. Monospace + icons — implemented

- **Font**: the whole app is monospace via `main.rs` `.default_font(iced::Font::MONOSPACE)`
  — the platform mono face (DejaVu/Menlo/Consolas), no bundled asset. To pin an
  *exact* face, bundle `Hack-Regular.ttf` with `include_bytes!` + `.font(...)`
  and set `iced::Font::with_name("Hack")` as the default. That's the only
  remaining upgrade for pixel-fidelity.
- **Icons**: emoji-presentation glyphs (⚡ `U+26A1`, 📑 `U+1F4D1`, screen `U+1F5B5`)
  are replaced with text-default codepoints (♪ ▤ ▣ ↻) so every icon renders
  monochrome in the text color. The rest (⚙ ✓ ✕ ⏻ ✎ arrows) already default to
  text presentation. No icon font needed.

> Status: §4 flat button + pick-list styles and the §3 tokens (4px radius) are
> ported into `theme.rs` and applied at every call site.

---

## 7. Accessibility check

- `text #f2e0cf` on `background #26201a` → very high contrast. ✅
- `text_muted #b2a08f` on base → ~7:1, fine for secondary text. ✅
- `background` (dark) text on `primary #fe8019` button → dark-on-orange,
  comfortable for button labels. ✅
- Don't put `primary` orange text on the `raised` surface for body copy — the
  contrast is borderline; use it for accents/large text only.
