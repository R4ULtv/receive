use crate::{
    auth::{API_ROOT, Credentials},
    model::{Attachment, DeliveryStatus, Draft, Email, Folder},
};
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use resend_rs::{
    ConfigBuilder, Method, Resend,
    list_opts::ListOptions,
    types::{
        CreateEmailBaseOptions, EmailEvent, GetInboundEmailOptions, InboundEmail,
        InboundEmailHtmlFormat,
    },
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::{Duration, Instant};

/// Resend's maximum page size, so a sync makes as few requests as it can.
const PAGE_SIZE: u8 = 100;

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Resend {}: {}", self.status, self.message)
    }
}

impl std::error::Error for ApiError {}

pub fn body_unavailable(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ApiError>()
        .is_some_and(|error| matches!(error.status, 404 | 410))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_missing_bodies_are_excluded_from_automatic_retries() {
        for code in [400, 401, 403, 404, 410, 429, 500, 503] {
            let error = anyhow::Error::from(ApiError {
                status: code,
                message: "test response".into(),
            })
            .context("archive request");
            assert_eq!(body_unavailable(&error), matches!(code, 404 | 410));
        }
        assert!(!body_unavailable(&anyhow::anyhow!("network error")));
    }

    /// Syncing walks Resend page by page, so a cursor that never reaches it
    /// would silently re-read the first page forever. The SDK spells its
    /// pagination with `serde(flatten)`, which URL encoding does not support
    /// everywhere, and the fallback in [`Api::read`] has to ask for the same
    /// page as the SDK does.
    #[test]
    fn pagination_reaches_resend_as_a_limit_and_a_cursor() {
        let client = http_client().unwrap();
        let query = |request: reqwest::blocking::Request| request.url().query().map(String::from);
        let first = query(
            client
                .get(API_ROOT)
                .query(&ListOptions::default().with_limit(PAGE_SIZE))
                .build()
                .unwrap(),
        );
        assert_eq!(first.as_deref(), Some("limit=100"));
        let next = query(
            client
                .get(API_ROOT)
                .query(
                    &ListOptions::default()
                        .with_limit(PAGE_SIZE)
                        .list_after("email-1"),
                )
                .build()
                .unwrap(),
        );
        assert_eq!(next.as_deref(), Some("limit=100&after=email-1"));
        let fallback = query(
            client
                .get(API_ROOT)
                .query(&page_query(Some("email-1")))
                .build()
                .unwrap(),
        );
        assert_eq!(fallback, next);
        assert_eq!(
            query(
                client
                    .get(API_ROOT)
                    .query(&page_query(None))
                    .build()
                    .unwrap()
            ),
            first
        );
    }

    /// Both the SDK and [`Api::read`] hand Resend a rooted path to join onto
    /// [`API_ROOT`], which silently drops anything already in its path.
    #[test]
    fn folder_endpoints_survive_being_joined_onto_the_api_root() {
        let root: reqwest::Url = API_ROOT.parse().unwrap();
        assert_eq!(
            root.join(Folder::Inbox.endpoint()).unwrap().as_str(),
            "https://api.resend.com/emails/receiving"
        );
        assert_eq!(
            root.join(Folder::Sent.endpoint()).unwrap().as_str(),
            "https://api.resend.com/emails"
        );
    }
}

#[derive(Deserialize)]
pub struct Page {
    #[serde(default)]
    pub data: Vec<Email>,
    #[serde(default)]
    pub has_more: bool,
}

pub struct Api {
    pub credentials: Credentials,
    pub client: Client,
    resend: Resend,
    next_request: Instant,
}

pub fn http_client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent("Receive/0.1.0")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

/// The official SDK, over Receive's own HTTP client and access token.
///
/// OAuth access tokens rotate and a [`Resend`] holds the one it was built
/// with, so this is called again whenever [`Credentials`] hand out a new one.
fn resend_client(access_token: &str, client: Client) -> Result<Resend> {
    Ok(Resend::with_config(
        ConfigBuilder::new(access_token)
            .base_url(API_ROOT.parse()?)
            .client(client)
            .build(),
    ))
}

/// Whether resend-rs could not read what Resend sent back.
fn unreadable<T>(result: &resend_rs::Result<T>) -> bool {
    matches!(result, Err(resend_rs::Error::Parse { .. }))
}

fn page_query(after: Option<&str>) -> Vec<(&'static str, String)> {
    let mut query = vec![("limit", PAGE_SIZE.to_string())];
    if let Some(cursor) = after {
        query.push(("after", cursor.to_owned()));
    }
    query
}

/// The SDK's delivery events in Receive's own vocabulary.
///
/// [`DeliveryStatus`] holds two states the SDK has no variant for: `Submitted`,
/// which is local to a message Receive has only just handed over, and
/// `Suppressed`, which only ever arrives over the wire (see [`Api::read`]).
fn delivery_status(event: EmailEvent) -> DeliveryStatus {
    match event {
        EmailEvent::Bounced => DeliveryStatus::Bounced,
        EmailEvent::Canceled => DeliveryStatus::Canceled,
        EmailEvent::Clicked => DeliveryStatus::Clicked,
        EmailEvent::Complained => DeliveryStatus::Complained,
        EmailEvent::Delivered => DeliveryStatus::Delivered,
        EmailEvent::DeliveryDelayed => DeliveryStatus::DeliveryDelayed,
        EmailEvent::Failed => DeliveryStatus::Failed,
        EmailEvent::Opened => DeliveryStatus::Opened,
        EmailEvent::Queued => DeliveryStatus::Queued,
        EmailEvent::Scheduled => DeliveryStatus::Scheduled,
        EmailEvent::Sent => DeliveryStatus::Sent,
    }
}

/// `folder`, `read` and `body_loaded` are Receive's own bookkeeping, and are
/// filled in by the caller that knows which request this message came from.
fn sent(email: resend_rs::types::Email) -> Email {
    Email {
        id: email.id.to_string(),
        from: email.from,
        to: email.to,
        cc: email.cc,
        reply_to: email.reply_to.unwrap_or_default(),
        subject: email.subject,
        created_at: email.created_at,
        text: email.text,
        html: email.html,
        message_id: email.message_id,
        last_event: Some(delivery_status(email.last_event)),
        ..Default::default()
    }
}

fn received(email: InboundEmail) -> Email {
    Email {
        id: email.id.to_string(),
        from: email.from,
        to: email.to,
        cc: email.cc,
        reply_to: email.reply_to,
        subject: email.subject,
        created_at: email.created_at,
        text: email.text,
        html: email.html,
        message_id: Some(email.message_id),
        headers: email.headers.into_iter().collect(),
        attachments: email
            .attachments
            .into_iter()
            .map(|attachment| Attachment {
                id: attachment.id.to_string(),
                filename: attachment.filename,
                content_type: attachment.content_type,
                size: attachment.size.unwrap_or_default().into(),
            })
            .collect(),
        ..Default::default()
    }
}

impl Api {
    pub fn new(credentials: Credentials) -> Result<Self> {
        let client = http_client()?;
        let resend = resend_client(&credentials.access_token, client.clone())?;
        Ok(Self {
            credentials,
            client,
            resend,
            next_request: Instant::now(),
        })
    }
    /// Renew the connection if it is due, and keep Receive's own spacing
    /// between requests: the SDK only paces itself on its async client.
    fn prepare(&mut self) -> Result<()> {
        self.credentials.refresh_if_needed(&self.client)?;
        let rotated = self.resend.api_key() != self.credentials.access_token.as_str();
        if rotated {
            self.resend = resend_client(&self.credentials.access_token, self.client.clone())?;
        }
        if self.next_request > Instant::now() {
            std::thread::sleep(self.next_request - Instant::now());
        }
        Ok(())
    }
    /// Space out the next request and put an SDK error into Receive's terms.
    fn finish<T>(&mut self, result: resend_rs::Result<T>) -> Result<T> {
        self.next_request = Instant::now() + Duration::from_millis(250);
        match result {
            Ok(value) => Ok(value),
            Err(resend_rs::Error::RateLimit {
                ratelimit_reset, ..
            }) => {
                let seconds = ratelimit_reset.unwrap_or(60).clamp(1, 300);
                self.next_request = Instant::now() + Duration::from_secs(seconds);
                bail!("Resend rate limit reached. Retrying after {seconds} seconds.")
            }
            Err(resend_rs::Error::Resend(response)) => Err(ApiError {
                status: response.status_code,
                message: response.message,
            }
            .into()),
            Err(error @ resend_rs::Error::Http(_)) => Err(anyhow::Error::from(error)
                .context("Could not reach Resend. Check your connection and try again.")),
            Err(error) => Err(error.into()),
        }
    }
    /// Take what the SDK read, or ask the same endpoint again and read it
    /// through Receive's own model.
    ///
    /// resend-rs types Resend's responses closely, down to a closed set of
    /// delivery events, so a status or a field it has not been taught about
    /// fails to parse and would take a whole folder with it. Raw requests are
    /// the SDK's documented way out of that, and [`Email`] was already written
    /// to tolerate missing, null and unrecognised values.
    fn read<T, U: DeserializeOwned>(
        &mut self,
        response: resend_rs::Result<T>,
        model: impl FnOnce(T) -> U,
        path: &str,
        query: Option<impl Serialize>,
    ) -> Result<U> {
        let unread = unreadable(&response);
        let response = self.finish(response);
        if unread {
            self.prepare()?;
            let raw = self
                .resend
                .send_raw(Method::GET, path, query, None::<()>, None);
            let raw = self.finish(raw)?;
            return serde_json::from_value(raw)
                .context("Resend sent a response Receive could not read");
        }
        Ok(model(response?))
    }
    pub fn list(&mut self, folder: Folder, after: Option<&str>) -> Result<Page> {
        let mut page = match folder {
            Folder::Inbox => self.inbox_page(after),
            Folder::Sent => self.sent_page(after),
        }?;
        for email in &mut page.data {
            email.folder = folder;
            email.read = folder == Folder::Sent;
            email.record_status_check();
        }
        Ok(page)
    }
    fn inbox_page(&mut self, after: Option<&str>) -> Result<Page> {
        self.prepare()?;
        let options = ListOptions::default().with_limit(PAGE_SIZE);
        let listed = match after {
            Some(cursor) => self.resend.receiving.list(options.list_after(cursor)),
            None => self.resend.receiving.list(options),
        };
        self.read(
            listed,
            |listed| Page {
                data: listed.data.into_iter().map(received).collect(),
                has_more: listed.has_more,
            },
            Folder::Inbox.endpoint(),
            Some(page_query(after)),
        )
    }
    fn sent_page(&mut self, after: Option<&str>) -> Result<Page> {
        self.prepare()?;
        let options = ListOptions::default().with_limit(PAGE_SIZE);
        let listed = match after {
            Some(cursor) => self.resend.emails.list(options.list_after(cursor)),
            None => self.resend.emails.list(options),
        };
        self.read(
            listed,
            |listed| Page {
                data: listed.data.into_iter().map(sent).collect(),
                has_more: listed.has_more,
            },
            Folder::Sent.endpoint(),
            Some(page_query(after)),
        )
    }
    pub fn email(&mut self, folder: Folder, id: &str) -> Result<Email> {
        let mut email = match folder {
            Folder::Inbox => self.inbox_email(id),
            Folder::Sent => self.sent_email(id),
        }?;
        email.folder = folder;
        email.body_loaded = true;
        email.record_status_check();
        if email.text.as_ref().is_none_or(|s| s.trim().is_empty()) && email.html.is_some() {
            email.text = Some(email.display_body());
        }
        Ok(email)
    }
    fn inbox_email(&mut self, id: &str) -> Result<Email> {
        self.prepare()?;
        // The reader displays text only; base64 inline images just inflate
        // downloads, the SQLite archive, and mailbox snapshots.
        let options =
            GetInboundEmailOptions::default().with_html_format(InboundEmailHtmlFormat::Cid);
        let fetched = self.resend.receiving.get(id, options);
        let path = format!("{}/{id}", Folder::Inbox.endpoint());
        self.read(fetched, received, &path, Some([("html_format", "cid")]))
    }
    fn sent_email(&mut self, id: &str) -> Result<Email> {
        self.prepare()?;
        let fetched = self.resend.emails.get(id);
        let path = format!("{}/{id}", Folder::Sent.endpoint());
        self.read(fetched, sent, &path, None::<()>)
    }
    /// Read the current domain configuration and required DNS records.
    pub fn domain_dns(&mut self, name: &str) -> Result<serde_json::Value> {
        self.prepare()?;
        // Omitting limit returns all domains according to the Resend API.
        let domains = self.resend.domains.list(ListOptions::default());
        let domains = self.finish(domains)?;
        let id = domains
            .data
            .iter()
            .find(|domain| domain.name.eq_ignore_ascii_case(name))
            .map(|domain| domain.id.to_string())
            .with_context(|| {
                format!("Domain {name} was not found in the connected Resend account")
            })?;
        self.prepare()?;
        let domain = self.resend.domains.get(&id);
        Ok(serde_json::to_value(self.finish(domain)?)?)
    }
    pub fn send(&mut self, draft: &Draft) -> Result<String> {
        draft.validate()?;
        let mut email = CreateEmailBaseOptions::new(
            draft.from.trim(),
            draft.recipients(),
            draft.subject.trim(),
        )
        .with_text(&draft.body);
        if let Some(message_id) = &draft.in_reply_to {
            email = email
                .with_header("In-Reply-To", message_id)
                .with_header("References", &draft.references);
        }
        let email = email.with_idempotency_key(&draft.idempotency_key);
        self.prepare()?;
        let sent = self.resend.emails.send(email);
        Ok(self.finish(sent)?.id.to_string())
    }
}
