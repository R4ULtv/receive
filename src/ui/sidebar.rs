//! The left rail.
//!
//! Icons only, so the rail costs as little width as possible. Every slot names
//! itself in a tooltip. Folders and domains are separate groups that combine:
//! Sent on `studio.dev` is the Sent slot and the `studio.dev` slot, both marked.

use super::{Receive, Screen, theme};
use crate::model::Folder;
use gpui_kit::assets::IconName;
use gpui_kit::component::{button::*, tooltip::Tooltip, *};
use gpui_kit::{prelude::*, *};
use std::path::PathBuf;

const WIDTH: Pixels = px(56.);
/// The square every slot occupies.
const SLOT: Pixels = px(36.);

impl Receive {
    pub(super) fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let domains = self.domains();
        let marks = monograms(domains.iter().map(|(domain, _)| domain.as_str()));
        let reading = self.screen == Screen::Mail;
        v_flex()
            .w(WIDTH)
            .flex_shrink_0()
            .h_full()
            .py_3()
            .gap_1()
            .items_center()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(theme::line(cx))
            .child(
                Button::new("compose")
                    .primary()
                    .icon(IconName::SquarePen)
                    .tooltip("Compose")
                    .size(SLOT)
                    .on_click(cx.listener(|this, _, _, cx| this.open(Screen::Compose, cx))),
            )
            .child(rule(cx))
            .child(self.slot(
                "inbox",
                "Inbox",
                self.unread(),
                reading && self.folder == Folder::Inbox,
                Icon::new(IconName::Inbox).small().into_any_element(),
                |this, _, cx| this.navigate(Folder::Inbox, cx),
                cx,
            ))
            .child(self.slot(
                "sent",
                "Sent",
                0,
                reading && self.folder == Folder::Sent,
                Icon::new(IconName::Send).small().into_any_element(),
                |this, _, cx| this.navigate(Folder::Sent, cx),
                cx,
            ))
            // One domain needs no picker: the folders already are the mailbox.
            .when(domains.len() > 1, |this| {
                this.child(rule(cx))
                    .child(self.slot(
                        "all-domains",
                        "All domains",
                        0,
                        self.domain.is_none(),
                        Icon::new(IconName::Globe).small().into_any_element(),
                        |this, _, cx| this.filter(None, cx),
                        cx,
                    ))
                    .children(
                        domains
                            .into_iter()
                            .zip(marks)
                            .map(|((domain, unread), mark)| {
                                let selected = self.domain.as_deref() == Some(domain.as_str());
                                let chosen = domain.clone();
                                let icon = self.icon_files.get(&domain).cloned();
                                self.slot(
                                    SharedString::from(format!("domain-{domain}")),
                                    domain.clone(),
                                    unread,
                                    selected,
                                    domain_chip(mark, icon, cx).into_any_element(),
                                    move |this, _, cx| this.filter(Some(chosen.clone()), cx),
                                    cx,
                                )
                            }),
                    )
            })
            .child(div().flex_1())
            .child(self.slot(
                "settings",
                "Settings",
                0,
                self.screen == Screen::Settings,
                Icon::new(IconName::Settings).small().into_any_element(),
                |this, _, cx| this.open(Screen::Settings, cx),
                cx,
            ))
    }

    /// One square of the rail: an icon or a monogram, a tooltip, an unread
    /// count, and a bar on the rail's edge when it is the one being shown.
    #[allow(clippy::too_many_arguments)]
    fn slot(
        &self,
        id: impl Into<ElementId>,
        tooltip: impl Into<SharedString>,
        unread: usize,
        selected: bool,
        content: AnyElement,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tooltip = tooltip.into();
        h_flex()
            .id(id)
            .relative()
            .size(SLOT)
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .text_color(if selected {
                cx.theme().foreground
            } else {
                theme::muted(cx)
            })
            .when(selected, |this| this.bg(cx.theme().sidebar_accent))
            .when(!selected, |this| {
                this.hover(|s| {
                    s.bg(cx.theme().list_hover)
                        .text_color(cx.theme().foreground)
                })
            })
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .child(content)
            .when(selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .left(px(-8.))
                        .w(px(3.))
                        .h(px(16.))
                        .rounded_full()
                        .bg(cx.theme().foreground),
                )
            })
            .when(unread > 0, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(-2.))
                        .right(px(-2.))
                        .min_w(px(16.))
                        .px(px(4.))
                        .rounded_full()
                        .bg(cx.theme().primary)
                        .text_color(cx.theme().primary_foreground)
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_center()
                        .child(compact(unread)),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .into_any_element()
    }
}

/// A domain stands for itself on a tile: its website's favicon once that has
/// been fetched, and its monogram until then or if it has none. The tile keeps
/// the domain group reading differently from the folders above it.
fn domain_chip(mark: String, icon: Option<PathBuf>, cx: &App) -> impl IntoElement {
    div()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(theme::line(cx))
        .bg(theme::surface(cx))
        .child(match icon {
            // A file that turns out not to decode falls back like a missing one.
            Some(icon) => img(icon)
                .size(px(18.))
                .with_fallback(move || monogram(mark.clone()).into_any_element())
                .into_any_element(),
            None => monogram(mark).into_any_element(),
        })
}

fn monogram(mark: String) -> impl IntoElement {
    div()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(mark)
}

/// The hairline that separates one group of slots from the next.
fn rule(cx: &App) -> impl IntoElement {
    div().my_2().w(px(20.)).h(px(1.)).bg(theme::line(cx))
}

/// The shortest monogram that still tells your domains apart: two letters
/// normally, three when two of them open the same way, so `raulcarini.dev` and
/// `railradar.app` read as `RAU` and `RAI` instead of two identical `RA`s. The
/// tooltip carries the full name either way.
fn monograms<'a>(domains: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    const SHORTEST: usize = 2;
    const LONGEST: usize = 3;

    let letters: Vec<Vec<char>> = domains
        .into_iter()
        .map(|domain| {
            domain
                .split('.')
                .next()
                .unwrap_or(domain)
                .chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_uppercase)
                .collect()
        })
        .collect();

    letters
        .iter()
        .enumerate()
        .map(|(index, own)| {
            let width = (SHORTEST..=LONGEST)
                .find(|&width| {
                    letters.iter().enumerate().all(|(other, theirs)| {
                        other == index || !theirs.iter().take(width).eq(own.iter().take(width))
                    })
                })
                .unwrap_or(LONGEST);
            let mark: String = own.iter().take(width).collect();
            if mark.is_empty() { "?".into() } else { mark }
        })
        .collect()
}

/// Unread counts stay inside their badge: anything past 99 is `99+`.
fn compact(count: usize) -> String {
    if count > 99 {
        "99+".into()
    } else {
        count.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::monograms;

    #[test]
    fn monograms_grow_only_as_far_as_telling_the_domains_apart_needs() {
        assert_eq!(
            monograms(["studio.dev", "yourdomain.com"]),
            ["ST", "YO"],
            "distinct openings stay at two letters"
        );
        assert_eq!(
            monograms(["railradar.app", "raulcarini.dev", "studio.dev"]),
            ["RAI", "RAU", "ST"],
            "only the domains that collide grow"
        );
        assert_eq!(
            monograms(["hi-keep.com", "hikeep.dev"]),
            ["HIK", "HIK"],
            "identical labels tie; the tooltip is what separates them"
        );
        assert_eq!(
            monograms(["x.dev", ""]),
            ["X", "?"],
            "no letters, no monogram"
        );
    }
}
