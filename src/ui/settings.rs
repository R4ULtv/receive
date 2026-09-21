//! Connecting an account, and what Receive keeps on this computer.

use super::{Receive, theme};
use crate::worker::Command;
use gpui_kit::assets::IconName;
use gpui_kit::component::{button::*, input::Input, *};
use gpui_kit::{prelude::*, *};

/// Settings reads as a column of prose, so it is kept to a comfortable measure.
const MEASURE: Pixels = px(620.);

impl Receive {
    pub(super) fn settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("settings")
            .flex_1()
            .h_full()
            .min_w_0()
            .overflow_y_scroll()
            .child(
                h_flex()
                    .h(theme::HEADER)
                    .flex_shrink_0()
                    .px_8()
                    .gap_2p5()
                    .items_center()
                    .border_b_1()
                    .border_color(theme::line(cx))
                    .child(Icon::new(IconName::Settings))
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Settings"),
                    ),
            )
            .child(
                v_flex()
                    .p_8()
                    .gap_5()
                    .max_w(MEASURE)
                    .child(self.account_card(cx))
                    .child(self.addresses_card(cx))
                    .child(self.storage_card(cx)),
            )
    }

    /// Signing in, and signing back out.
    fn account_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        card(cx)
            .child(card_header(
                IconName::AtSign,
                "Your Resend account",
                "Sign in through your browser. Resend currently requires Full access to read your inbox.",
                cx,
            ))
            .child(
                Button::new("oauth")
                    .primary()
                    .icon(if self.connected {
                        IconName::CircleCheck
                    } else {
                        IconName::ShieldCheck
                    })
                    .label(if self.connected {
                        "Account connected"
                    } else {
                        "Connect Resend"
                    })
                    .loading(self.busy && !self.connected)
                    .disabled(self.busy || self.connected)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preview = false;
                        this.busy = true;
                        this.error = None;
                        this.command(Command::ConnectOAuth);
                        cx.notify();
                    })),
            )
            .when(self.connected, |this| {
                this.child(
                    Button::new("disconnect")
                        .outline()
                        .label("Disconnect this account")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.save_draft(cx);
                            this.command(Command::Disconnect);
                            cx.notify();
                        })),
                )
            })
            .when(!self.connected, |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .pt_4()
                        .gap_3()
                        .border_t_1()
                        .border_color(theme::line(cx))
                        .child(theme::section_label("Or use a Full access API key", cx))
                        .child(Input::new(&self.key).disabled(self.busy).prefix(
                            Icon::new(IconName::KeyRound)
                                .small()
                                .text_color(theme::muted(cx)),
                        ))
                        .child(
                            Button::new("connect-key")
                                .outline()
                                .label("Use API key")
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let key = this.key.read(cx).value().to_string();
                                    if !key.trim().starts_with("re_") {
                                        this.error =
                                            Some("Enter a Resend API key starting with re_.".into());
                                    } else {
                                        this.preview = false;
                                        this.busy = true;
                                        this.error = None;
                                        this.command(Command::ConnectKey(key));
                                    }
                                    cx.notify();
                                })),
                        ),
                )
            })
    }

    /// The addresses you want offered in every draft, whether or not you have
    /// written to them before.
    fn addresses_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let editable = self.connected && !self.busy;
        card(cx)
            .child(card_header(
                IconName::AtSign,
                "Saved addresses",
                "From and To offer everyone you have corresponded with. Addresses saved here are offered as well, before the rest.",
                cx,
            ))
            .when(!self.contacts.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .gap_1()
                        .children(self.contacts.clone().into_iter().map(|address| {
                            let saved = address.clone();
                            h_flex()
                                .w_full()
                                .h(px(36.))
                                .pl_3()
                                .pr_1()
                                .gap_2()
                                .items_center()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().background)
                                .border_1()
                                .border_color(theme::line(cx))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .child(address.clone()),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("forget-{address}")))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::X)
                                        .tooltip("Forget this address")
                                        .disabled(!editable)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_contact(&saved, cx)
                                        })),
                                )
                        })),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.contact).disabled(!editable).prefix(
                                Icon::new(IconName::AtSign)
                                    .small()
                                    .text_color(theme::muted(cx)),
                            ),
                        ),
                    )
                    .child(
                        Button::new("save-address")
                            .outline()
                            .icon(IconName::Plus)
                            .label("Save")
                            .disabled(!editable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_contact(window, cx)
                            })),
                    ),
            )
            .when(!self.connected, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme::muted(cx))
                        .child("Connect an account to save addresses."),
                )
            })
    }

    /// Where the mail lives once it has been downloaded.
    fn storage_card(&self, cx: &App) -> impl IntoElement + use<> {
        card(cx)
            .child(card_header(
                IconName::HardDrive,
                "On this computer",
                "Receive checks for new mail every 60 seconds while it is open. Downloaded messages stay here, so open the app now and then to save them before Resend's 30-day retention window ends.",
                cx,
            ))
            .child(
                v_flex()
                    .w_full()
                    .gap_1p5()
                    .child(theme::section_label("Local storage", cx))
                    .child(
                        div()
                            .p_3()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().background)
                            .border_1()
                            .border_color(theme::line(cx))
                            .text_xs()
                            .text_color(theme::muted(cx))
                            .child(self.data_dir.display().to_string()),
                    ),
            )
            .child(
                Button::new("resend-dashboard")
                    .ghost()
                    .small()
                    .icon(IconName::ExternalLink)
                    .label("Open the Resend dashboard")
                    .on_click(|_, _, cx| cx.open_url("https://resend.com/domains")),
            )
    }
}

/// The surface each group of settings sits on.
fn card(cx: &App) -> Div {
    v_flex()
        .p_6()
        .gap_4()
        .items_start()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(theme::line(cx))
        .bg(cx.theme().popover)
}

fn card_header(
    icon: IconName,
    title: &'static str,
    detail: &'static str,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_2()
        .child(
            h_flex()
                .gap_2p5()
                .items_center()
                .child(Icon::new(icon).small())
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_sm()
                        .child(title),
                ),
        )
        .child(div().text_sm().text_color(theme::muted(cx)).child(detail))
}
