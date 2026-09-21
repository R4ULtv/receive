//! The two mail panes: the list of messages and the message being read.

use super::{Receive, Screen, View, long_date, short_date, theme};
use crate::{
    model::{Email, Folder},
    worker::Command,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::*,
    input::{Input, Textarea},
    tooltip::Tooltip,
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
        let threads = self.visible(cx);
        let count = threads.len();
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
                                            .child(self.view.name()),
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
                            .filter_map(|index| threads.get(index).copied())
                            .map(|thread| this.thread_row(thread, cx))
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
            "No conversations match your search."
        } else if self.view == View::Archive {
            "Conversations you archive are kept here. Archiving is local: Resend still has them."
        } else if self.view == View::Sent {
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

    /// One conversation in the list.
    ///
    /// A conversation is named by everyone who wrote in it, carries the
    /// subject it started with, and previews the message that arrived last.
    fn thread_row(&self, thread: usize, cx: &mut Context<Self>) -> AnyElement {
        let messages = self.conversation(thread);
        let (Some(&first), Some(&last)) = (messages.first(), messages.last()) else {
            // A conversation with nothing in view is filtered out before here.
            return div().h(ROW_HEIGHT).into_any_element();
        };
        let (oldest, newest) = (&self.emails[first], &self.emails[last]);
        let key = self.threads[thread].key.clone();
        let selected = self.selected.as_deref() == Some(key.as_str());
        let unread = messages.iter().any(|&index| !self.emails[index].read);
        let archived = self.view == View::Archive;
        let count = messages.len();
        let attachments = messages
            .iter()
            .any(|&index| !self.emails[index].attachments.is_empty());
        // Every name that wrote, oldest first, so a conversation reads as the
        // exchange it is rather than as its most recent message.
        let correspondents = if self.view == View::Sent {
            newest
                .to
                .iter()
                .map(|address| super::display_name(address))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            let mut names: Vec<String> = Vec::new();
            for &index in &messages {
                let name = super::display_name(&self.emails[index].from);
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            names.join(", ")
        };
        let subject = if oldest.subject.trim().is_empty() {
            "(No subject)".to_string()
        } else {
            oldest.subject.clone()
        };
        // Row controls are revealed by hovering the row they belong to, so the
        // list stays a list until it is being used.
        let group = SharedString::from(format!("row-{key}"));
        let hint = theme::line(cx);

        v_flex()
            .id(SharedString::from(format!("thread-{key}")))
            .group(group.clone())
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
            .on_click(cx.listener({
                let key = key.clone();
                move |this, _, window, cx| this.select(key.clone(), window, cx)
            }))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        // The unread mark is also the control that sets it.
                        div()
                            .id(SharedString::from(format!("read-{key}")))
                            .flex_shrink_0()
                            .size(px(14.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .tooltip(move |window, cx| {
                                Tooltip::new(if unread {
                                    "Mark as read"
                                } else {
                                    "Mark as unread"
                                })
                                .build(window, cx)
                            })
                            .on_click(cx.listener({
                                let key = key.clone();
                                move |this, _, _, cx| {
                                    // Otherwise the row underneath opens the
                                    // conversation and reads it straight back.
                                    cx.stop_propagation();
                                    this.toggle_read(key.clone(), cx);
                                }
                            }))
                            .child(
                                div()
                                    .size(px(6.))
                                    .rounded_full()
                                    .bg(if unread {
                                        cx.theme().foreground
                                    } else {
                                        cx.theme().transparent
                                    })
                                    .when(!unread, |this| {
                                        this.group_hover(group.clone(), move |s| s.bg(hint))
                                    }),
                            ),
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
                            .child(correspondents),
                    )
                    .when(count > 1, |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .px_1p5()
                                .rounded(cx.theme().radius)
                                .bg(theme::surface(cx))
                                .text_xs()
                                .text_color(theme::muted(cx))
                                .child(count.to_string()),
                        )
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme::muted(cx))
                            .child(short_date(&newest.created_at)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .opacity(0.)
                            .group_hover(group.clone(), |s| s.opacity(1.))
                            .child(
                                Button::new(SharedString::from(format!("archive-{key}")))
                                    .ghost()
                                    .small()
                                    .icon(if archived {
                                        IconName::ArchiveRestore
                                    } else {
                                        IconName::Archive
                                    })
                                    .tooltip(if archived {
                                        "Move back to its folder"
                                    } else {
                                        "Archive on this computer"
                                    })
                                    .on_click(cx.listener({
                                        let key = key.clone();
                                        move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.archive_thread(key.clone(), !archived, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .pl(px(14.))
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(subject))
                    .when(attachments, |this| {
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
                    .child(div().flex_1().min_w_0().truncate().child(newest.preview()))
                    .when(newest.folder == Folder::Sent, |this| {
                        this.child(delivery_badge(newest, cx))
                    }),
            )
            .into_any_element()
    }

    /// The conversation being read: its messages in order, with one of them
    /// open. Both folders are shown, so a reply sits with what it answered.
    pub(super) fn reader(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(thread) = self.selected_thread() else {
            return self.reader_placeholder(cx).into_any_element();
        };
        let messages = self.conversation(thread);
        let Some(open) = self.open_email() else {
            return self.reader_placeholder(cx).into_any_element();
        };
        let key = self.threads[thread].key.clone();
        let email = &self.emails[open];
        let archived = self.view == View::Archive;
        let count = messages.len();
        let position = messages
            .iter()
            .position(|&index| index == open)
            .unwrap_or_default();
        let subject = {
            let oldest = &self.emails[messages[0]];
            if oldest.subject.trim().is_empty() {
                "(No subject)".to_string()
            } else {
                oldest.subject.clone()
            }
        };
        let url = email.dashboard_url();

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
                            .child(match (self.preview, count) {
                                (true, 1) => "Sample".to_string(),
                                (true, count) => format!("Sample · {count} messages"),
                                (false, 1) => "Message".to_string(),
                                (false, count) => format!("Conversation · {count} messages"),
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("mark-unread")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Mail)
                                    .tooltip(if count > 1 {
                                        "Mark the conversation unread"
                                    } else {
                                        "Mark as unread"
                                    })
                                    .on_click(cx.listener({
                                        let key = key.clone();
                                        move |this, _, _, cx| {
                                            this.mark_thread_unread(key.clone(), cx)
                                        }
                                    })),
                            )
                            .child(
                                Button::new("archive")
                                    .ghost()
                                    .small()
                                    .icon(if archived {
                                        IconName::ArchiveRestore
                                    } else {
                                        IconName::Archive
                                    })
                                    .tooltip(if archived {
                                        "Move back to its folder"
                                    } else {
                                        "Archive on this computer. Resend keeps the message."
                                    })
                                    .on_click(cx.listener({
                                        let key = key.clone();
                                        move |this, _, _, cx| {
                                            this.archive_thread(key.clone(), !archived, cx)
                                        }
                                    })),
                            )
                            .child({
                                // Whatever Receive leaves out — the original
                                // HTML, the raw source, the attachments — is a
                                // click away on the dashboard. Sample messages
                                // have invented ids and no page to open.
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
                                            || email.folder == Folder::Sent,
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
                            .pb_4()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(subject),
                            )
                            .when(archived, |this| {
                                this.child(
                                    div()
                                        .pt_1p5()
                                        .text_xs()
                                        .text_color(theme::muted(cx))
                                        .child("Archived on this computer. Resend still has it."),
                                )
                            }),
                    )
                    .child(self.folded("earlier-messages", &messages[..position], cx))
                    .child(self.open_message_pane(open, count == 1, cx))
                    .child(self.folded("later-messages", &messages[position + 1..], cx)),
            )
            .into_any_element()
    }

    /// The messages of the conversation that are not the one being read.
    /// A long exchange scrolls rather than crowding out the message itself.
    fn folded(&self, id: &'static str, messages: &[usize], cx: &mut Context<Self>) -> AnyElement {
        if messages.is_empty() {
            return div().into_any_element();
        }
        v_flex()
            .id(id)
            .flex_shrink_0()
            .max_h(px(168.))
            .overflow_y_scroll()
            .children(
                messages
                    .iter()
                    .map(|&index| self.folded_row(index, cx))
                    .collect::<Vec<_>>(),
            )
            .into_any_element()
    }

    fn folded_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let email = &self.emails[index];
        let id = email.id.clone();
        h_flex()
            .id(SharedString::from(format!("folded-{id}")))
            .h(px(44.))
            .flex_shrink_0()
            .px_8()
            .gap_3()
            .items_center()
            .border_t_1()
            .border_color(theme::line(cx))
            .cursor_pointer()
            .hover(|s| s.bg(cx.theme().list_hover))
            .on_click(cx.listener(move |this, _, window, cx| this.show(id.clone(), window, cx)))
            .child(self.correspondent(&email.from, px(22.), cx))
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .child(super::display_name(&email.from)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme::muted(cx))
                    .child(email.preview()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme::muted(cx))
                    .child(short_date(&email.created_at)),
            )
            .into_any_element()
    }

    /// The one message of the conversation shown in full.
    fn open_message_pane(&self, index: usize, alone: bool, cx: &mut Context<Self>) -> AnyElement {
        let email = &self.emails[index];
        let id = email.id.clone();
        let archived = self.view == View::Archive;
        v_flex()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(theme::line(cx))
            .child(
                v_flex()
                    .px_8()
                    .pt_5()
                    .pb_4()
                    .gap_4()
                    .flex_shrink_0()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(self.correspondent(&email.from, px(36.), cx))
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
                            )
                            // Filing one message of an exchange away, rather
                            // than the whole of it. On its own the header
                            // button already says the same thing.
                            .when(!alone, |this| {
                                this.child(
                                    Button::new("archive-message")
                                        .ghost()
                                        .small()
                                        .icon(if archived {
                                            IconName::ArchiveRestore
                                        } else {
                                            IconName::Archive
                                        })
                                        .tooltip(if archived {
                                            "Move this message back"
                                        } else {
                                            "Archive this message only"
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.archive_message(id.clone(), !archived, cx)
                                        })),
                                )
                            }),
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
            })
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
