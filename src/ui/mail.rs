//! The two mail panes: the list of messages and the message being read.

use super::{Receive, Screen, avatar, long_date, short_date, theme};
use crate::{
    model::{Email, Folder},
    worker::Command,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::*,
    input::{Input, Textarea},
    *,
};
use gpui_kit::{prelude::*, *};

const LIST_WIDTH: Pixels = px(360.);
const ROW_HEIGHT: Pixels = px(104.);
/// A message is read as a column, not as one long line across a wide window.
const MEASURE: Pixels = px(820.);
/// The search field is the pane's one control, so it is given a full-size row
/// rather than the compact height a toolbar would use.
const SEARCH_HEIGHT: Pixels = px(36.);

impl Receive {
    pub(super) fn message_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let emails = self.visible(cx);
        let count = emails.len();
        let searching = !self.search.read(cx).value().is_empty();
        v_flex()
            .w(LIST_WIDTH)
            .min_w(px(280.))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(theme::line(cx))
            .child(
                h_flex()
                    .h(theme::HEADER)
                    .flex_shrink_0()
                    .px_5()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_baseline()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(self.folder.name()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::muted(cx))
                                            .child(count.to_string()),
                                    ),
                            )
                            // Which of your domains is being shown, when it is
                            // not all of them.
                            .when_some(self.domain.clone(), |this, domain| {
                                this.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(theme::muted(cx))
                                        .child(domain),
                                )
                            }),
                    )
                    .child(
                        Button::new("refresh")
                            .ghost()
                            .small()
                            .icon(IconName::RotateCw)
                            .tooltip("Refresh mail and delivery statuses")
                            .loading(self.busy)
                            .disabled(!self.connected || self.busy || self.preview)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.error = None;
                                this.command(Command::Sync);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div().px_4().pb_4().child(
                    Input::new(&self.search)
                        .h(SEARCH_HEIGHT)
                        .cleanable(true)
                        .prefix(
                            Icon::new(IconName::Search)
                                .small()
                                .text_color(theme::muted(cx)),
                        ),
                ),
            )
            .child(if count == 0 {
                self.list_placeholder(searching, cx).into_any_element()
            } else {
                uniform_list(
                    "mail-list",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|index| this.emails.get(emails[index]))
                            .map(|email| this.message_row(email, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .into_any_element()
            })
    }

    /// What the list says when it has nothing to show.
    fn list_placeholder(&self, searching: bool, cx: &App) -> impl IntoElement {
        let message = if searching {
            "No messages match your search."
        } else if self.folder == Folder::Sent {
            "Messages you send appear here."
        } else if self.connected || self.preview {
            "No messages here yet."
        } else {
            "Your inbox will appear here."
        };
        v_flex()
            .flex_1()
            .px_6()
            .pt_8()
            .items_center()
            .text_sm()
            .text_color(theme::muted(cx))
            .child(message)
    }

    fn message_row(&self, email: &Email, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let selected = self.selected.as_deref() == Some(&email.id);
        let id = email.id.clone();
        let correspondent = if self.folder == Folder::Sent {
            email
                .to
                .iter()
                .map(|address| super::display_name(address))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            super::display_name(&email.from)
        };
        let subject = if email.subject.trim().is_empty() {
            "(No subject)".to_string()
        } else {
            email.subject.clone()
        };
        let unread = !email.read;

        v_flex()
            .id(SharedString::from(format!("email-{}", email.id)))
            .h(ROW_HEIGHT)
            .w_full()
            .px_5()
            .py_4()
            .gap_1p5()
            .justify_center()
            .border_l_2()
            .border_color(if selected {
                cx.theme().foreground
            } else {
                cx.theme().transparent
            })
            .when(selected, |this| this.bg(theme::surface(cx)))
            .when(!selected, |this| {
                this.hover(|s| s.bg(cx.theme().list_hover))
            })
            .cursor_pointer()
            .overflow_hidden()
            .on_click(cx.listener(move |this, _, window, cx| this.select(id.clone(), window, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .size(px(6.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(if unread {
                                cx.theme().foreground
                            } else {
                                cx.theme().transparent
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(if unread {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(correspondent),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme::muted(cx))
                            .child(short_date(&email.created_at)),
                    ),
            )
            .child(
                h_flex()
                    .pl(px(14.))
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(subject))
                    .when(!email.attachments.is_empty(), |this| {
                        this.child(
                            Icon::new(IconName::Paperclip)
                                .xsmall()
                                .text_color(theme::muted(cx)),
                        )
                    }),
            )
            .child(
                h_flex()
                    .pl(px(14.))
                    .gap_2()
                    .items_center()
                    .text_xs()
                    .text_color(theme::muted(cx))
                    .child(div().flex_1().min_w_0().truncate().child(email.preview()))
                    .when(email.folder == Folder::Sent, |this| {
                        this.child(delivery_badge(email, cx))
                    }),
            )
    }

    pub(super) fn reader(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(email) = self.selected_email() else {
            return self.reader_placeholder(cx).into_any_element();
        };
        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .child(
                h_flex()
                    .h(theme::HEADER)
                    .flex_shrink_0()
                    .px_6()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::line(cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme::muted(cx))
                            .child(Icon::new(IconName::Mail).xsmall())
                            .child(if self.preview { "Sample" } else { "Message" }),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child({
                                // Whatever Receive leaves out — the original
                                // HTML, the raw source, the attachments — is a
                                // click away on the dashboard. Sample messages
                                // have invented ids and no page to open.
                                let url = email.dashboard_url();
                                Button::new("open-on-resend")
                                    .ghost()
                                    .small()
                                    .icon(IconName::ExternalLink)
                                    .tooltip("Open this message on the Resend dashboard")
                                    .disabled(self.preview)
                                    .on_click(move |_, _, cx| cx.open_url(&url))
                            })
                            .child(
                                Button::new("reply")
                                    .outline()
                                    .small()
                                    .icon(IconName::Reply)
                                    .label("Reply")
                                    .disabled(
                                        self.busy
                                            || !email.body_loaded
                                            || self.folder == Folder::Sent,
                                    )
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.reply(window, cx)),
                                    ),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .max_w(MEASURE)
                    .child(
                        v_flex()
                            .px_8()
                            .pt_7()
                            .pb_5()
                            .gap_5()
                            .flex_shrink_0()
                            .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child(
                                if email.subject.trim().is_empty() {
                                    "(No subject)".to_string()
                                } else {
                                    email.subject.clone()
                                },
                            ))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(avatar(&email.from, px(36.), cx))
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_0p5()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_sm()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(email.from.clone()),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(theme::muted(cx))
                                                    .child(format!("To {}", email.to.join(", "))),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(theme::muted(cx))
                                            .child(long_date(&email.created_at)),
                                    ),
                            )
                            .when(email.folder == Folder::Sent, |this| {
                                this.child(self.delivery_details(email, cx))
                            }),
                    )
                    .child(
                        div().px_7().pb_6().flex_1().min_h_0().child(
                            Textarea::new(&self.reader)
                                .readonly(true)
                                .appearance(false)
                                .h_full(),
                        ),
                    )
                    .when(!email.attachments.is_empty(), |this| {
                        this.child(self.attachments(email, cx))
                    }),
            )
            .into_any_element()
    }

    /// Attachment names. Receive shows what arrived; downloading is not
    /// implemented yet, so these are labels rather than buttons.
    fn attachments(&self, email: &Email, cx: &App) -> impl IntoElement + use<> {
        let names: Vec<String> = email
            .attachments
            .iter()
            .map(|a| a.filename.clone().unwrap_or_else(|| "Unnamed file".into()))
            .collect();
        v_flex()
            .flex_shrink_0()
            .px_8()
            .py_4()
            .gap_2p5()
            .border_t_1()
            .border_color(theme::line(cx))
            .child(theme::section_label("Attachments", cx))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(names.into_iter().map(|name| {
                        h_flex()
                            .px_2p5()
                            .py_1()
                            .gap_2()
                            .items_center()
                            .rounded(cx.theme().radius)
                            .bg(theme::surface(cx))
                            .text_xs()
                            .child(
                                Icon::new(IconName::Paperclip)
                                    .xsmall()
                                    .text_color(theme::muted(cx)),
                            )
                            .child(name)
                    })),
            )
    }

    fn delivery_details(&self, email: &Email, cx: &App) -> impl IntoElement + use<> {
        v_flex()
            .p_3()
            .gap_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(theme::line(cx))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .flex_wrap()
                    .child(delivery_badge(email, cx))
                    .when_some(email.status_checked_at.as_ref(), |this, checked| {
                        this.child(div().text_xs().text_color(theme::muted(cx)).child(
                            if self.preview {
                                "Sample status".into()
                            } else {
                                format!("Last checked {}", long_date(checked))
                            },
                        ))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted(cx))
                    .child(email.delivery_status().description()),
            )
    }

    /// The reader with nothing selected: the first thing a new account sees.
    fn reader_placeholder(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let opened = self.connected || self.preview;
        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .items_center()
            .justify_center()
            .p_8()
            .gap_4()
            .child(
                div()
                    .size(px(56.))
                    .rounded(px(16.))
                    .bg(theme::surface(cx))
                    .border_1()
                    .border_color(theme::line(cx))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if opened {
                        Icon::new(IconName::MailOpen).size(px(24.))
                    } else {
                        super::mark(px(24.))
                    }),
            )
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if opened {
                        "Nothing open"
                    } else {
                        "Your mail, on your machine"
                    }),
            )
            .child(
                div()
                    .max_w(px(360.))
                    .text_center()
                    .text_sm()
                    .text_color(theme::muted(cx))
                    .child(if opened {
                        "Pick a message from the list to read it here."
                    } else {
                        "A small, native home for your Resend email. Connect an account, or look around with sample messages first."
                    }),
            )
            .when(!opened, |this| {
                this.child(
                    h_flex()
                        .pt_2()
                        .gap_2()
                        .child(
                            Button::new("get-started")
                                .primary()
                                .label("Connect your email")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.open(Screen::Settings, cx)),
                                ),
                        )
                        .child(Button::new("preview").ghost().label("Take a look around").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.show_preview(window, cx);
                                cx.notify();
                            }),
                        )),
                )
            })
    }
}

/// Status is always named as well as marked by an icon/color.
fn delivery_badge(email: &Email, cx: &App) -> impl IntoElement + use<> {
    let status = email.delivery_status();
    let color = if status.needs_attention() {
        cx.theme().danger
    } else if status.is_positive() {
        cx.theme().foreground
    } else {
        theme::muted(cx)
    };
    let icon = if status.needs_attention() {
        IconName::TriangleAlert
    } else if status.is_positive() {
        IconName::CircleCheck
    } else {
        IconName::Clock
    };
    h_flex()
        .flex_shrink_0()
        .gap_1()
        .items_center()
        .text_xs()
        .text_color(color)
        .child(Icon::new(icon).xsmall())
        .child(status.label())
}
