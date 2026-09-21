//! The appearance of the application.
//!
//! Receive is monochrome: black surfaces, white text, and one shade of red kept
//! for errors. The palette lives in `assets/theme.json` so it can be read in one
//! place, and it is applied to GPUI Component's theme rather than to individual
//! elements, so buttons, inputs, scrollbars, and menus match the rest of the
//! window without repeating colors at every call site.

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeConfig, ThemeMode};
use gpui_kit::{
    App, FontWeight, Hsla, IntoElement, ParentElement as _, Pixels, Styled as _, div, px,
};
use std::rc::Rc;

const PALETTE: &str = include_str!("../../assets/theme.json");

/// Install the palette. Call once, after `gpui_kit::init`.
pub fn install(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let config: ThemeConfig =
        serde_json::from_str(PALETTE).expect("assets/theme.json is not a valid theme");
    Theme::global_mut(cx).apply_config(&Rc::new(config));
    Theme::sync_base(cx);
}

/// Text that is present but not the point: timestamps, counts, hints.
pub fn muted(cx: &App) -> Hsla {
    cx.theme().muted_foreground
}

/// Hairlines between panes and rows.
pub fn line(cx: &App) -> Hsla {
    cx.theme().border
}

/// A raised surface on the black background: cards, chips, avatars.
pub fn surface(cx: &App) -> Hsla {
    cx.theme().secondary
}

/// The height shared by the header of every pane, so the panes line up.
pub const HEADER: Pixels = px(60.);

/// A quiet heading over a group of rows or fields.
pub fn section_label(label: &'static str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(muted(cx))
        .child(label)
}
