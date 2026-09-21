use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Resend uses both omitted fields and JSON null for optional mail metadata.
/// `serde(default)` alone only handles omission, not explicit null.
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// The Resend dashboard, which shows a message in full when Receive only holds
/// part of it.
pub const DASHBOARD: &str = "https://resend.com";

/// Resend returns RFC 3339 and PostgreSQL-style timestamps on different endpoints.
pub fn parse_date(value: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .or_else(|_| chrono::DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%#z"))
        .ok()
}

pub fn sort_emails(emails: &mut [Email]) {
    emails.sort_by_cached_key(|email| {
        std::cmp::Reverse(parse_date(&email.created_at).map(|date| date.timestamp_micros()))
    });
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Folder {
    #[default]
    Inbox,
    Sent,
}

impl Folder {
    pub fn name(self) -> &'static str {
        match self {
            Self::Inbox => "Inbox",
            Self::Sent => "Sent",
        }
    }
    pub fn endpoint(self) -> &'static str {
        match self {
            Self::Inbox => "/emails/receiving",
            Self::Sent => "/emails",
        }
    }
}

/// Resend's latest email event, plus a local state after POST /emails succeeds.
/// Accepting a message for processing is not confirmation of delivery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Submitted,
    Queued,
    Scheduled,
    Sent,
    Delivered,
    DeliveryDelayed,
    Bounced,
    Failed,
    Suppressed,
    Complained,
    Opened,
    Clicked,
    Canceled,
    #[default]
    #[serde(other)]
    Unknown,
}

impl DeliveryStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Submitted => "Submitted",
            Self::Queued => "Queued",
            Self::Scheduled => "Scheduled",
            Self::Sent => "Sent",
            Self::Delivered => "Delivered",
            Self::DeliveryDelayed => "Delayed",
            Self::Bounced => "Bounced",
            Self::Failed => "Failed",
            Self::Suppressed => "Suppressed",
            Self::Complained => "Spam complaint",
            Self::Opened => "Opened",
            Self::Clicked => "Clicked",
            Self::Canceled => "Canceled",
            Self::Unknown => "Status unknown",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Submitted => "Resend accepted this message. Waiting for a delivery update.",
            Self::Queued => "Resend has queued this message for sending.",
            Self::Scheduled => "This message is scheduled to be sent later.",
            Self::Sent => "Resend sent this message. Delivery is not yet confirmed.",
            Self::Delivered => {
                "The recipient's mail server accepted this message. This does not confirm it was read."
            }
            Self::DeliveryDelayed => "Delivery is temporarily delayed. Resend is retrying.",
            Self::Bounced => {
                "The recipient's mail server rejected this message. Check the Resend dashboard for the bounce reason."
            }
            Self::Failed => {
                "Resend could not send this message. Check the Resend dashboard for details."
            }
            Self::Suppressed => {
                "Resend blocked delivery because the recipient is on its suppression list."
            }
            Self::Complained => "A spam complaint was reported for this message.",
            Self::Opened => {
                "Resend recorded an open event. Tracking does not guarantee a person read the message."
            }
            Self::Clicked => {
                "Resend recorded a link click. Tracking may include automated activity."
            }
            Self::Canceled => "This scheduled message was canceled before sending.",
            Self::Unknown => "No recognized delivery status is available for this message yet.",
        }
    }

    pub fn needs_attention(self) -> bool {
        matches!(
            self,
            Self::Bounced | Self::Failed | Self::Suppressed | Self::Complained
        )
    }

    pub fn is_positive(self) -> bool {
        matches!(self, Self::Delivered | Self::Opened | Self::Clicked)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub filename: Option<String>,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Email {
    pub id: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub cc: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub reply_to: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub subject: String,
    #[serde(default)]
    pub created_at: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub message_id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "null_default")]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub folder: Folder,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub body_loaded: bool,
    #[serde(default)]
    pub last_event: Option<DeliveryStatus>,
    /// When the API last returned this status, not when the delivery event happened.
    #[serde(default)]
    pub status_checked_at: Option<String>,
}

impl Email {
    pub fn delivery_status(&self) -> DeliveryStatus {
        self.last_event.unwrap_or_default()
    }

    pub fn record_status_check(&mut self) {
        if self.folder == Folder::Sent && self.last_event.is_some() {
            self.status_checked_at = Some(chrono::Utc::now().to_rfc3339());
        }
    }

    pub fn display_body(&self) -> String {
        if let Some(text) = self.text.as_ref().filter(|s| !s.trim().is_empty()) {
            return text.clone();
        }
        if let Some(html) = &self.html {
            return html2text::from_read(html.as_bytes(), 100)
                .unwrap_or_else(|_| "This message could not be converted to text.".into());
        }
        if self.body_loaded {
            "This email has no text content.".into()
        } else {
            "Message content has not been downloaded yet.".into()
        }
    }
    pub fn preview(&self) -> String {
        self.text
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .enumerate()
            .flat_map(|(index, word)| (index > 0).then_some(' ').into_iter().chain(word.chars()))
            .take(110)
            .collect()
    }
    pub fn reply_subject(&self) -> String {
        if self.subject.to_lowercase().starts_with("re:") {
            self.subject.clone()
        } else {
            format!("Re: {}", self.subject)
        }
    }
    pub fn reply_recipient(&self) -> String {
        self.reply_to
            .first()
            .cloned()
            .unwrap_or_else(|| self.from.clone())
    }
    /// The domains this message is filed under: the addresses it arrived at for
    /// received mail, and the sender's own domain for mail that was sent. A
    /// message addressed to two of your domains belongs to both.
    pub fn domains(&self) -> Vec<String> {
        let mut domains: Vec<String> = match self.folder {
            Folder::Inbox => self
                .to
                .iter()
                .chain(self.cc.iter())
                .filter_map(|address| domain_of(address))
                .collect(),
            Folder::Sent => domain_of(&self.from).into_iter().collect(),
        };
        domains.sort();
        domains.dedup();
        domains
    }
    /// This message on the Resend dashboard, where everything Receive does not
    /// show — the original HTML, the raw source, the attachments themselves —
    /// can still be read.
    ///
    /// The two folders sit under different paths, the same way their API
    /// endpoints do. These are dashboard addresses, not API ones, so they are
    /// written out rather than borrowed from [`Folder::endpoint`]: if Resend
    /// moves a dashboard page, only this should follow it.
    pub fn dashboard_url(&self) -> String {
        let page = match self.folder {
            Folder::Sent => "emails",
            Folder::Inbox => "emails/receiving",
        };
        format!("{DASHBOARD}/{page}/{}", self.id)
    }
    pub fn references(&self) -> String {
        let previous = self
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("references"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        format!("{} {}", previous, self.message_id.as_deref().unwrap_or(""))
            .trim()
            .into()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub in_reply_to: Option<String>,
    pub references: String,
    pub idempotency_key: String,
}

impl Draft {
    /// Whether this draft can be sent.
    ///
    /// Sender verification belongs to Resend. Domains observed in mail are not
    /// a complete list of verified sending domains (and can include other
    /// people's addresses in To/CC).
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            valid_address(&self.from),
            "Enter a sender on your verified Resend domain."
        );
        anyhow::ensure!(
            !self.recipients().is_empty(),
            "Enter at least one recipient."
        );
        anyhow::ensure!(
            self.recipients().iter().all(|s| valid_address(s)),
            "Check the recipient addresses (separate them with semicolons)."
        );
        anyhow::ensure!(!self.subject.trim().is_empty(), "Enter a subject.");
        anyhow::ensure!(
            !self.subject.contains(['\r', '\n']),
            "The subject must be a single line."
        );
        anyhow::ensure!(
            !self.body.trim().is_empty(),
            "Write a message before sending."
        );
        Ok(())
    }
    pub fn recipients(&self) -> Vec<String> {
        self.to
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }
}

/// Every domain these messages are filed under, in alphabetical order. This is
/// what the account is known to use, which is as close to its verified domains
/// as Receive can get without asking Resend for them.
pub fn domains_of(emails: &[Email]) -> Vec<String> {
    let mut domains: Vec<String> = emails.iter().flat_map(Email::domains).collect();
    domains.sort();
    domains.dedup();
    domains
}

/// The address inside `Maya Chen <maya@example.com>`, or the whole value when
/// it carries no display name.
fn bare_address(value: &str) -> &str {
    value
        .rsplit_once('<')
        // Trimmed before the bracket is stripped, or trailing space after the
        // `>` leaves it attached and the domain comes out as `example.com>`.
        .map(|(_, rest)| rest.trim().trim_end_matches('>'))
        .unwrap_or(value)
        .trim()
}

/// The domain an address belongs to, lowercased: `example.com`. A value that is
/// not a usable mail address has no domain.
pub fn domain_of(value: &str) -> Option<String> {
    Some(mailbox_of(value)?.rsplit_once('@')?.1.to_string())
}

/// The mailbox an address names, lowercased and without its display name, so
/// that `Raul <contact@example.com>` and `contact@example.com` are recognised as
/// one address rather than two.
pub fn mailbox_of(value: &str) -> Option<String> {
    valid_address(value).then(|| bare_address(value).to_lowercase())
}

fn valid_address(value: &str) -> bool {
    if value.chars().any(char::is_control) {
        return false;
    }
    let value = value.trim();
    if value.contains(['<', '>'])
        && !(value.matches('<').count() == 1
            && value.matches('>').count() == 1
            && value.ends_with('>'))
    {
        return false;
    }
    let address = bare_address(value);
    address.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && !local.contains(['<', '>', ',', ';'])
            && !address.contains(char::is_whitespace)
            && domain.contains('.')
            && domain.len() <= 253
            && domain.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            })
    })
}

pub fn demo_messages() -> Vec<Email> {
    // Two receiving domains, so the sample inbox shows how mail is filed.
    let mut messages: Vec<Email> = [
        ("Maya Chen <maya@example.com>", "you@yourdomain.com", "A quieter place for your email", "Hey,\n\nWelcome to Receive. Your domains, your conversations, one small desktop app.\n\nThis is a sample message. Connect your Resend account in Settings to load your real inbox.\n\nMail is checked every 60 seconds while the app is open, and messages are saved locally for later.\n\nMaya", false),
        ("Alex Rivera <alex@example.com>", "you@yourdomain.com", "Re: The next small thing", "I like the direction. Let's keep the first version focused: reading, writing, and staying out of the way.\n\nTalk soon,\nAlex", false),
        ("Studio North <hello@example.com>", "hello@studio.dev", "Friday notes", "A few things for next week:\n\n1. Review the first design\n2. Try the inbox\n3. Make something useful\n\nHave a good weekend.", true),
    ].into_iter().enumerate().map(|(i, (from, to, subject, body, read))| Email {
        id: format!("demo-{i}"), from: from.into(), to: vec![to.into()], subject: subject.into(),
        text: Some(body.into()), created_at: (chrono::Utc::now() - chrono::Duration::hours(i as i64 * 3)).to_rfc3339(),
        message_id: Some(format!("<demo-{i}@example.com>")), read, body_loaded: true, ..Default::default()
    }).collect();
    for (id, status, subject) in [
        (
            "demo-delivered",
            DeliveryStatus::Delivered,
            "The updated notes",
        ),
        (
            "demo-bounced",
            DeliveryStatus::Bounced,
            "A message that couldn't be delivered",
        ),
        (
            "demo-delayed",
            DeliveryStatus::DeliveryDelayed,
            "Waiting for the mail server",
        ),
    ] {
        messages.push(Email {
            id: id.into(),
            from: "You <you@yourdomain.com>".into(),
            to: vec!["Maya Chen <maya@example.com>".into()],
            subject: subject.into(),
            text: Some("This is a sample sent message. Its delivery status is shown in the list and above the message.".into()),
            created_at: chrono::Utc::now().to_rfc3339(),
            folder: Folder::Sent,
            read: true,
            body_loaded: true,
            last_event: Some(status),
            status_checked_at: Some(chrono::Utc::now().to_rfc3339()),
            ..Default::default()
        });
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn api_dates_display_and_sort_by_instant_instead_of_text() {
        let dates = [
            "2026-04-03T22:13:42.674981Z",
            "2026-04-03 22:13:42.674981+00",
            "2026-04-04T00:13:42.674981+02:00",
        ];
        let expected = parse_date(dates[0]).unwrap();
        for date in dates {
            assert_eq!(parse_date(date).unwrap(), expected);
        }
        let mut emails: Vec<_> = [
            "2026-04-03T10:00:00Z",
            "2026-04-03 22:00:00+00",
            "2026-04-03T12:00:00Z",
        ]
        .into_iter()
        .map(|date| Email {
            created_at: date.into(),
            ..Default::default()
        })
        .collect();
        sort_emails(&mut emails);
        assert_eq!(emails[0].created_at, "2026-04-03 22:00:00+00");
        assert_eq!(emails[2].created_at, "2026-04-03T10:00:00Z");
    }
    #[test]
    fn sent_mail_accepts_documented_null_recipient_metadata() {
        let page: crate::api::Page = serde_json::from_value(serde_json::json!({
            "object": "list", "has_more": false,
            "data": [{"id": "sent-1", "from": "me@example.com", "to": ["you@example.com"],
                "subject": "Hello", "created_at": "2026-04-03 22:13:42.674981+00",
                "bcc": null, "cc": null, "reply_to": null, "last_event": "delivered"}]
        }))
        .unwrap();
        assert_eq!(page.data.len(), 1);
        let email = &page.data[0];
        assert!(email.cc.is_empty() && email.reply_to.is_empty());
        assert_eq!(email.reply_recipient(), "me@example.com");
        assert_eq!(email.last_event, Some(DeliveryStatus::Delivered));
    }

    #[test]
    fn missing_null_and_future_statuses_never_claim_delivery() {
        for value in [
            serde_json::json!({"id": "old-cache"}),
            serde_json::json!({"id": "null", "last_event": null}),
            serde_json::json!({"id": "future", "last_event": "new_resend_event"}),
        ] {
            let email: Email = serde_json::from_value(value).unwrap();
            assert_eq!(email.delivery_status(), DeliveryStatus::Unknown);
            assert!(!email.delivery_status().is_positive());
        }
        assert!(!DeliveryStatus::Submitted.is_positive());
        assert!(!DeliveryStatus::Sent.is_positive());
    }

    #[test]
    fn status_checks_are_only_recorded_for_observed_sent_statuses() {
        let mut email = Email {
            folder: Folder::Sent,
            ..Default::default()
        };
        email.record_status_check();
        assert!(email.status_checked_at.is_none());
        email.last_event = Some(DeliveryStatus::Bounced);
        email.record_status_check();
        assert!(email.status_checked_at.is_some());
        assert!(email.delivery_status().needs_attention());
        let mut inbox = Email {
            folder: Folder::Inbox,
            last_event: Some(DeliveryStatus::Delivered),
            ..Default::default()
        };
        inbox.record_status_check();
        assert!(inbox.status_checked_at.is_none());
    }

    #[test]
    fn optional_metadata_accepts_null_missing_and_populated_values() {
        let null: Email = serde_json::from_value(serde_json::json!({
            "id": "1", "cc": null, "reply_to": null, "subject": null,
            "headers": null, "attachments": null, "text": null, "html": null
        }))
        .unwrap();
        let missing: Email = serde_json::from_value(serde_json::json!({"id": "1"})).unwrap();
        assert_eq!(
            serde_json::to_value(null).unwrap(),
            serde_json::to_value(missing).unwrap()
        );
        let populated: Email = serde_json::from_value(serde_json::json!({
            "id": "1", "cc": ["cc@example.com"], "reply_to": ["reply@example.com"],
            "headers": {"references": "<previous>"}, "attachments": [{"id": "attachment-1"}]
        }))
        .unwrap();
        assert_eq!(populated.reply_recipient(), "reply@example.com");
        assert_eq!(populated.cc.len(), 1);
        assert_eq!(populated.headers["references"], "<previous>");
        assert_eq!(populated.attachments.len(), 1);
    }
    #[test]
    fn mail_is_filed_under_the_domain_it_arrived_at_or_was_sent_from() {
        let received = Email {
            folder: Folder::Inbox,
            from: "Someone <someone@elsewhere.test>".into(),
            to: vec![
                "you@first.example".into(),
                "You <you@Second.Example>".into(),
            ],
            cc: vec!["team@first.example".into()],
            ..Default::default()
        };
        assert_eq!(received.domains(), ["first.example", "second.example"]);

        let sent = Email {
            folder: Folder::Sent,
            from: "You <you@second.example>".into(),
            to: vec!["someone@elsewhere.test".into()],
            ..Default::default()
        };
        assert_eq!(sent.domains(), ["second.example"]);

        let unusable = Email {
            folder: Folder::Inbox,
            to: vec!["undisclosed-recipients".into(), String::new()],
            ..Default::default()
        };
        assert!(unusable.domains().is_empty());
    }

    #[test]
    fn replies_use_reply_to_and_keep_references() {
        let email = Email {
            from: "sender@example.com".into(),
            reply_to: vec!["reply@example.com".into()],
            subject: "Re: Meeting".into(),
            message_id: Some("<new>".into()),
            headers: BTreeMap::from([("References".into(), "<old>".into())]),
            ..Default::default()
        };
        assert_eq!(email.reply_recipient(), "reply@example.com");
        assert_eq!(email.references(), "<old> <new>");
        assert_eq!(email.reply_subject(), "Re: Meeting");
    }
    #[test]
    fn html_only_mail_is_readable_without_loading_remote_resources() {
        let email = Email {
            html: Some("<p>Hello <b>there</b></p><script>alert(1)</script>".into()),
            ..Default::default()
        };
        let body = email.display_body();
        assert!(body.contains("Hello"));
        assert!(!body.contains("alert(1)"));
    }
    #[test]
    fn draft_rejects_header_injection_and_accepts_display_names() {
        let mut draft = Draft {
            from: "Receive <me@example.com>".into(),
            to: "\"Doe, Jane\" <jane@example.com>; second@example.com".into(),
            subject: "Hello".into(),
            body: "Message".into(),
            ..Default::default()
        };
        assert!(draft.validate().is_ok());
        assert_eq!(draft.recipients().len(), 2);
        draft.from = "me@example.com\r\nBcc: bad@example.com".into();
        assert!(draft.validate().is_err());
    }

    #[test]
    fn a_message_points_at_its_own_page_on_the_dashboard() {
        let sent = Email {
            folder: Folder::Sent,
            id: "01a0b538-b251-71df-95c9-10476563f167".into(),
            ..Default::default()
        };
        assert_eq!(
            sent.dashboard_url(),
            "https://resend.com/emails/01a0b538-b251-71df-95c9-10476563f167"
        );

        let received = Email {
            folder: Folder::Inbox,
            id: "4ef9a417-02e9-4d39-ad75-9611e0fcb13c".into(),
            ..Default::default()
        };
        assert_eq!(
            received.dashboard_url(),
            "https://resend.com/emails/receiving/4ef9a417-02e9-4d39-ad75-9611e0fcb13c"
        );
    }

    #[test]
    fn one_mailbox_is_one_address_however_it_is_written() {
        let same = [
            "contact@example.com",
            "Contact@Example.com",
            "Raul <contact@example.com>",
            "  Raul Carini <contact@example.com>  ",
        ];
        for written in same {
            assert_eq!(
                mailbox_of(written).as_deref(),
                Some("contact@example.com"),
                "{written:?}"
            );
        }
        assert_eq!(
            mailbox_of("hello@example.com").as_deref(),
            Some("hello@example.com")
        );
        assert_eq!(mailbox_of("undisclosed-recipients"), None);
    }

    #[test]
    fn a_new_sending_domain_does_not_require_existing_mail() {
        let draft = Draft {
            from: "Receive <hello@mine.example>".into(),
            to: "someone@elsewhere.test".into(),
            subject: "Hello".into(),
            body: "Message".into(),
            ..Default::default()
        };
        assert!(draft.validate().is_ok());

        let borrowed = Draft {
            from: "hello@notmine.example".into(),
            ..draft.clone()
        };
        assert!(borrowed.validate().is_ok());
    }

    #[test]
    fn malformed_mailboxes_cannot_supply_url_paths_or_headers() {
        for address in [
            "a@b@example.com",
            "a@example.com/path",
            "a@example.com:443",
            "a@example.com?query",
            "a@example..com",
            "a@-example.com",
            "Name <a@example.com",
            "a@example.com>",
            "a@example.com\0",
        ] {
            assert_eq!(domain_of(address), None, "{address:?}");
        }
    }

    #[test]
    fn previews_are_bounded_and_preserve_unicode() {
        let email = Email {
            text: Some(format!("  hello\nworld\t{}", "é".repeat(200))),
            ..Default::default()
        };
        let preview = email.preview();
        assert!(preview.starts_with("hello world é"));
        assert_eq!(preview.chars().count(), 110);
    }

    #[test]
    fn the_domains_of_a_mailbox_are_every_domain_its_messages_are_filed_under() {
        let emails = vec![
            Email {
                folder: Folder::Inbox,
                to: vec!["you@first.example".into()],
                ..Default::default()
            },
            Email {
                folder: Folder::Sent,
                from: "You <you@second.example>".into(),
                to: vec!["someone@elsewhere.test".into()],
                ..Default::default()
            },
            Email {
                folder: Folder::Inbox,
                to: vec!["another@first.example".into()],
                ..Default::default()
            },
        ];
        assert_eq!(domains_of(&emails), ["first.example", "second.example"]);
    }
}
