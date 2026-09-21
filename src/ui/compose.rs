//! The draft editor.
//!
//! From and To offer addresses while you type in them: the ones saved in
//! Settings, and the ones Receive works out from your mail. From offers only
//! previously used sender addresses. Resend validates sending domains.

use super::{Field, Receive, Screen, theme};
use crate::worker::Command;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::*,
    input::{Input, Textarea},
    *,
};
use gpui_kit::{prelude::*, *};

/// The width of the field labels, so the inputs start on one line.
const LABEL: Pixels = px(64.);
/// The height of a row in the address list under a field.
const SUGGESTION: Pixels = px(32.);

impl Receive {
    pub(super) fn compose(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let replying = self.draft.in_reply_to.is_some();
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
                            .gap_2p5()
                            .items_center()
                            .child(Icon::new(if replying {
                                IconName::Reply
                            } else {
                                IconName::SquarePen
                            }))
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(if replying { "Reply" } else { "New message" }),
                            ),
                    )
                    .child(
                        Button::new("close-compose")
                            .ghost()
                            .small()
                            .icon(IconName::X)
                            .tooltip("Close the draft and keep it saved")
                            .on_click(cx.listener(|this, _, _, cx| this.open(Screen::Mail, cx))),
                    ),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .px_6()
                    .pt_5()
                    .gap_3()
                    .child(self.address_field(Field::From, "From", cx))
                    .child(self.address_field(Field::To, "To", cx))
                    .child(field(
                        "Subject",
                        Input::new(&self.subject).disabled(self.busy),
                        cx,
                    )),
            )
            .child(div().mt_5().h(px(1.)).flex_shrink_0().bg(theme::line(cx)))
            .child(
                div().px_6().py_4().flex_1().min_h_0().child(
                    Textarea::new(&self.body)
                        .appearance(false)
                        .h_full()
                        .disabled(self.busy),
                ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .px_6()
                    .py_4()
                    .gap_4()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(theme::line(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme::muted(cx))
                            .child(if self.preview {
                                "Preview mode · sending is disabled"
                            } else if !self.connected {
                                "Connect an account in Settings to send"
                            } else {
                                "Plain text · draft saved on this computer"
                            }),
                    )
                    .child(
                        Button::new("send")
                            .primary()
                            .icon(IconName::Send)
                            .label("Send message")
                            .loading(self.busy)
                            .disabled(!self.connected || self.preview || self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let draft = this.current_draft(cx);
                                match draft.validate() {
                                    Ok(()) => {
                                        this.draft = draft.clone();
                                        this.dirty_since = None;
                                        this.busy = true;
                                        this.error = None;
                                        this.command(Command::Send(draft));
                                    }
                                    Err(error) => this.error = Some(error.to_string()),
                                }
                                cx.notify();
                            })),
                    ),
            )
    }

    /// A draft field that offers addresses under itself while it has focus.
    ///
    /// The arrow keys move through the list, Enter takes the highlighted one,
    /// and Escape puts it away. Those keys are caught above the field so that
    /// they reach the list instead of moving the text cursor.
    fn address_field(
        &self,
        which: Field,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = match which {
            Field::From => &self.from,
            Field::To => &self.to,
        };
        let suggestions = if !self.busy && self.suggesting == Some(which) {
            self.suggestions(which, cx)
        } else {
            Vec::new()
        };
        let highlighted = self.highlighted.min(suggestions.len().saturating_sub(1));
        div()
            .id(SharedString::from(format!("field-{label}")))
            .relative()
            .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if this.busy || this.suggesting != Some(which) {
                    return;
                }
                let options = this.suggestions(which, cx);
                if options.is_empty() {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "down" => this.highlight(1, options.len()),
                    "up" => this.highlight(-1, options.len()),
                    "escape" => this.suggesting = None,
                    "enter" => {
                        if let Some(chosen) = this.highlighted_of(&options) {
                            this.accept(which, &chosen, window, cx);
                        }
                    }
                    _ => return,
                }
                // The field must not also act on the key: an arrow would move
                // the text cursor and Enter would end the list a second time.
                cx.stop_propagation();
                cx.notify();
            }))
            .child(field(label, Input::new(state).disabled(self.busy), cx))
            .when(!suggestions.is_empty(), |this| {
                // The list hangs below the row without moving the fields under
                // it, and is deferred so it paints over them rather than behind.
                this.child(deferred(
                    div()
                        .absolute()
                        .top_full()
                        .left(LABEL + px(12.))
                        .right_0()
                        .child(
                            v_flex()
                                .mt_1()
                                .py_1()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(theme::line(cx))
                                .bg(cx.theme().popover)
                                .shadow_lg()
                                .children(suggestions.into_iter().enumerate().map(
                                    |(index, address)| {
                                        let chosen = address.clone();
                                        h_flex()
                                            .id(SharedString::from(format!(
                                                "suggest-{label}-{address}"
                                            )))
                                            .h(SUGGESTION)
                                            .px_3()
                                            .gap_2()
                                            .items_center()
                                            .cursor_pointer()
                                            // Marked the way a chosen message is
                                            // marked in the list: a bar on the
                                            // edge, not a tint to be looked for.
                                            .border_l_2()
                                            .border_color(if index == highlighted {
                                                cx.theme().foreground
                                            } else {
                                                cx.theme().transparent
                                            })
                                            .when(index == highlighted, |s| s.bg(cx.theme().accent))
                                            .hover(|s| s.bg(cx.theme().accent))
                                            .child(self.address_mark(&address, cx))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_sm()
                                                    .child(address),
                                            )
                                            // Taken on the press, not the
                                            // release: pressing here takes focus
                                            // off the field, which closes the
                                            // list, and a click that is held for
                                            // even a moment would land on
                                            // nothing.
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, window, cx| {
                                                    this.accept(which, &chosen, window, cx);
                                                    // Without this the window
                                                    // goes on to clear focus,
                                                    // undoing the field being
                                                    // focused again above.
                                                    window.prevent_default();
                                                }),
                                            )
                                    },
                                )),
                        ),
                ))
            })
    }
}

/// One labelled row of the draft header.
fn field(label: &'static str, input: Input, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_3()
        .items_center()
        .child(
            div()
                .w(LABEL)
                .flex_shrink_0()
                .text_sm()
                .text_color(theme::muted(cx))
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(input))
}
