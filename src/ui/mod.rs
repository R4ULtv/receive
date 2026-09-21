//! The Receive window.
//!
//! [`Receive`] owns every piece of interface state and talks to the mail worker
//! through a channel. Each pane renders from that state in its own module:
//!
//! | Module      | Pane                                        |
//! | ----------- | ------------------------------------------- |
//! | [`sidebar`] | Compose, folders, domains                   |
//! | [`mail`]    | Message list and reader                     |
//! | [`compose`] | Draft editor                                |
//! | [`settings`]| Connection and storage                      |

pub mod assets;
mod compose;
mod mail;
mod settings;
mod sidebar;
pub mod theme;

use crate::{
    favicon::Icons,
    model::{Draft, Email, Folder, demo_messages},
    thread::Thread,
    worker::{Command, Event, Worker},
};
use chrono::Datelike as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::*,
    input::{InputEvent, InputState, TextareaState},
    spinner::Spinner,
    tooltip::Tooltip,
    *,
};
use gpui_kit::{prelude::*, *};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{Duration, Instant},
};

/// How long typing settles before the draft is written to the database.
const DRAFT_DEBOUNCE: Duration = Duration::from_millis(450);
/// How often worker events are drained and timers are checked.
const TICK: Duration = Duration::from_millis(100);
/// How many addresses a field offers at once.
const SUGGESTIONS: usize = 6;
/// How long a suggestion list outlives its field losing focus, so that a click
/// on one of the suggestions is not cut off by the list closing.
const SUGGESTION_GRACE: Duration = Duration::from_millis(180);

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Mail,
    Compose,
    Settings,
}

/// Which conversations the list shows.
///
/// Inbox and Sent are Resend's two folders. Archive is Receive's own shelf and
/// cuts across both: it holds whatever has been filed away, wherever it came
/// from. Nothing here is a deletion — Resend still has every message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    Inbox,
    Sent,
    Archive,
}

impl View {
    fn name(self) -> &'static str {
        match self {
            Self::Inbox => "Inbox",
            Self::Sent => "Sent",
            Self::Archive => "Archive",
        }
    }
}

/// A draft field that offers addresses while you type in it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    From,
    To,
}

pub struct Receive {
    worker: Worker,
    /// The favicon of each domain, once its website has answered for it.
    icons: Icons,
    icon_files: BTreeMap<String, PathBuf>,
    asked_for_icon: BTreeSet<String>,
    icons_dirty: bool,
    data_dir: PathBuf,
    emails: Vec<Email>,
    /// [`Self::emails`] grouped into conversations, rebuilt whenever the mail
    /// changes rather than on every frame.
    threads: Vec<Thread>,
    view: View,
    /// The domain the list is narrowed to, or every domain when this is `None`.
    domain: Option<String>,
    /// Addresses saved by hand in Settings.
    contacts: Vec<String>,
    /// The draft field currently showing its suggestions, if any.
    suggesting: Option<Field>,
    /// Which of those suggestions the arrow keys are on.
    highlighted: usize,
    closing_suggestions: Option<Instant>,
    screen: Screen,
    /// The key of the conversation being read.
    selected: Option<String>,
    /// The id of the message shown open inside it. Without one, the newest
    /// message of the conversation is the one open.
    open_message: Option<String>,
    connected: bool,
    preview: bool,
    busy: bool,
    closing: bool,
    error: Option<String>,
    status: String,
    search: Entity<InputState>,
    key: Entity<InputState>,
    /// The Settings field for adding an address to [`Self::contacts`].
    contact: Entity<InputState>,
    from: Entity<InputState>,
    to: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    reader: Entity<TextareaState>,
    draft: Draft,
    dirty_since: Option<Instant>,
    _subscriptions: Vec<Subscription>,
    smoke_test: bool,
    smoke_frames: u8,
    smoke_ticks: u32,
}

impl Receive {
    pub fn new(
        data_dir: PathBuf,
        preview: bool,
        smoke_test: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search your mail"));
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("re_…").masked(true));
        let contact =
            cx.new(|cx| InputState::new(window, cx).placeholder("Name <someone@example.com>"));
        let from = cx.new(|cx| InputState::new(window, cx).placeholder("You <you@yourdomain.com>"));
        let to =
            cx.new(|cx| InputState::new(window, cx).placeholder("Recipient; another@example.com"));
        let subject =
            cx.new(|cx| InputState::new(window, cx).placeholder("A subject for your message"));
        let body = cx.new(|cx| TextareaState::new(window, cx).placeholder("Write something…"));
        let reader = cx.new(|cx| TextareaState::new(window, cx));

        let mut subscriptions = vec![cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify())];
        for (field, offers) in [
            (&from, Some(Field::From)),
            (&to, Some(Field::To)),
            (&subject, None),
        ] {
            subscriptions.push(cx.subscribe(field, move |this, _, event, cx| {
                match event {
                    // Editing or arriving in the field offers addresses again,
                    // starting from the top of the list.
                    InputEvent::Change | InputEvent::Focus => {
                        if matches!(event, InputEvent::Change) {
                            this.dirty_since = Some(Instant::now());
                        }
                        this.suggesting = offers;
                        this.highlighted = 0;
                        this.closing_suggestions = None;
                        this.icons_dirty = true;
                    }
                    // Pressing a suggestion takes focus off the field. The list
                    // is closed a moment later so that press is not cut off.
                    InputEvent::Blur => this.closing_suggestions = Some(Instant::now()),
                    InputEvent::PressEnter { .. } => this.suggesting = None,
                }
                cx.notify();
            }));
        }
        subscriptions.push(cx.subscribe(&body, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.dirty_since = Some(Instant::now());
            }
            cx.notify();
        }));

        let view = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            view.update(cx, |this, cx| this.request_close(cx))
                .unwrap_or(true)
        });

        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this
                    .update_in(cx, |this, window, cx| {
                        this.handle_events(window, cx);
                        this.handle_icons(cx);
                        this.close_suggestions(cx);
                        this.advance_smoke_test(window, cx);
                        if this
                            .dirty_since
                            .is_some_and(|since| since.elapsed() >= DRAFT_DEBOUNCE)
                        {
                            this.save_draft(cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();

        let mut result = Self {
            worker: Worker::start(data_dir.clone(), !preview),
            icons: Icons::start(data_dir.clone()),
            icon_files: BTreeMap::new(),
            asked_for_icon: BTreeSet::new(),
            icons_dirty: true,
            data_dir,
            emails: Vec::new(),
            threads: Vec::new(),
            view: View::Inbox,
            domain: None,
            contacts: Vec::new(),
            suggesting: None,
            highlighted: 0,
            closing_suggestions: None,
            screen: Screen::Mail,
            selected: None,
            open_message: None,
            connected: false,
            preview: false,
            busy: false,
            closing: false,
            error: None,
            status: "Ready when you are".into(),
            search,
            key,
            contact,
            from,
            to,
            subject,
            body,
            reader,
            draft: Draft::default(),
            dirty_since: None,
            _subscriptions: subscriptions,
            smoke_test,
            smoke_frames: 0,
            smoke_ticks: 0,
        };
        if preview {
            result.show_preview(window, cx);
        }
        result
    }

    fn command(&mut self, command: Command) {
        if self.worker.commands.send(command).is_err() {
            self.busy = false;
            self.closing = false;
            self.error = Some("The mail worker stopped. Restart Receive to reconnect.".into());
        }
    }

    fn request_close(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.connected || self.preview {
            return true;
        }
        if !self.closing {
            // Sending already persists its exact draft. A close queued behind
            // a send must not restore that draft after successful delivery.
            let draft = (!self.busy).then(|| self.current_draft(cx));
            self.closing = true;
            self.busy = true;
            self.status = "Saving before closing…".into();
            self.command(Command::FlushDraft(draft));
            cx.notify();
        }
        false
    }

    fn handle_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let events: Vec<_> = self.worker.events.try_iter().collect();
        if events.is_empty() {
            return;
        }
        self.icons_dirty = true;
        for event in events {
            match event {
                Event::Connected {
                    emails,
                    draft,
                    contacts,
                } => {
                    self.connected = true;
                    self.preview = false;
                    self.emails = emails;
                    self.selected = None;
                    self.open_message = None;
                    self.domain = None;
                    self.contacts = contacts;
                    self.screen = Screen::Mail;
                    self.error = None;
                    self.key
                        .update(cx, |state, cx| state.set_value("", window, cx));
                    self.fill_draft(draft, window, cx);
                }
                Event::Disconnected => {
                    self.connected = false;
                    self.emails.clear();
                    self.selected = None;
                    self.open_message = None;
                    self.domain = None;
                    self.contacts.clear();
                    self.icon_files.clear();
                    self.asked_for_icon.clear();
                    self.screen = Screen::Settings;
                    self.fill_draft(Draft::default(), window, cx);
                }
                Event::Snapshot(emails) => self.emails = emails,
                Event::Updated(email) => {
                    if self.open_message.as_deref() == Some(&email.id) {
                        self.reader.update(cx, |state, cx| {
                            state.set_value(email.display_body(), window, cx)
                        });
                    }
                    if let Some(existing) = self
                        .emails
                        .iter_mut()
                        .find(|e| e.id == email.id && e.folder == email.folder)
                    {
                        *existing = *email;
                    } else {
                        self.emails.push(*email);
                        crate::model::sort_emails(&mut self.emails);
                    }
                }
                Event::Sent => {
                    let from = self.from.read(cx).value().to_string();
                    self.fill_draft(
                        Draft {
                            from,
                            ..Default::default()
                        },
                        window,
                        cx,
                    );
                    self.screen = Screen::Mail;
                    self.view = View::Sent;
                    self.selected = None;
                    self.open_message = None;
                    self.error = None;
                }
                Event::Status(status) => self.status = status,
                Event::Error(error) => {
                    self.error = Some(error);
                    self.closing = false;
                }
                Event::Busy(busy) => self.busy = busy || self.closing,
                Event::DraftFlushed => {
                    // A failed in-flight send cancels closing, so its error
                    // stays visible and unsaved fields can be saved on retry.
                    if self.closing {
                        window.remove_window();
                        return;
                    }
                }
            }
        }
        self.rebuild_threads();
        cx.notify();
    }

    /// Collect the icons that have arrived, and ask about domains not asked
    /// about yet. Sample domains are somebody else's websites, so preview mode
    /// asks about nothing and keeps its monograms.
    fn handle_icons(&mut self, cx: &mut Context<Self>) {
        let mut arrived = false;
        for (domain, icon) in self.icons.found.try_iter().collect::<Vec<_>>() {
            self.icon_files.insert(domain, icon);
            arrived = true;
        }
        if self.connected && !self.preview && self.icons_dirty {
            self.icons_dirty = false;
            let mut wanted: Vec<String> =
                self.domains().into_iter().map(|(name, _)| name).collect();
            // The people you write to are other people's domains, so Receive
            // only asks about them while you are actually composing, when their
            // icons are about to be drawn.
            if self.screen == Screen::Compose {
                wanted.extend(
                    self.suggesting
                        .map(|field| self.suggestions(field, cx))
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|address| crate::model::domain_of(address)),
                );
            }
            // The people who wrote to you are other people's domains too. The
            // reader puts a face on each message of the conversation that is
            // open, so ask for those and no others.
            if self.screen == Screen::Mail {
                wanted.extend(
                    self.selected_thread()
                        .map(|thread| self.conversation(thread))
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|index| crate::model::domain_of(&self.emails[index].from)),
                );
            }
            for domain in wanted {
                if self.asked_for_icon.insert(domain.clone()) {
                    let _ = self.icons.wanted.send(domain);
                }
            }
        }
        if arrived {
            cx.notify();
        }
    }

    /// The mark shown beside an address: its domain's favicon once Receive has
    /// it, and a plain glyph until then or if the domain has none.
    fn address_mark(&self, address: &str, cx: &App) -> AnyElement {
        let icon = crate::model::domain_of(address)
            .and_then(|domain| self.icon_files.get(&domain).cloned());
        let muted = theme::muted(cx);
        match icon {
            Some(icon) => img(icon)
                .size(px(16.))
                .with_fallback(move || glyph(muted).into_any_element())
                .into_any_element(),
            None => glyph(muted).into_any_element(),
        }
    }

    /// The face beside a message: the sender's domain favicon once Receive
    /// has it, and the initial of their name until then, or if the domain
    /// has none. [`Self::address_mark`] is the same idea at list size.
    fn correspondent(&self, address: &str, size: Pixels, cx: &App) -> AnyElement {
        let icon = crate::model::domain_of(address)
            .and_then(|domain| self.icon_files.get(&domain).cloned());
        let letter = initial(address);
        div()
            .size(size)
            .flex_shrink_0()
            .rounded_full()
            .bg(theme::surface(cx))
            .border_1()
            .border_color(theme::line(cx))
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .child(match icon {
                // A file that turns out not to decode falls back like a
                // missing one, the same way the domain chips do.
                Some(icon) => img(icon)
                    .size(size * 0.62)
                    .with_fallback(move || div().child(letter.clone()).into_any_element())
                    .into_any_element(),
                None => div().child(letter).into_any_element(),
            })
            .into_any_element()
    }

    fn current_draft(&self, cx: &App) -> Draft {
        let mut draft = Draft {
            from: self.from.read(cx).value().to_string(),
            to: self.to.read(cx).value().to_string(),
            subject: self.subject.read(cx).value().to_string(),
            body: self.body.read(cx).value().to_string(),
            ..self.draft.clone()
        };
        if draft != self.draft || draft.idempotency_key.is_empty() {
            draft.idempotency_key = uuid::Uuid::new_v4().to_string();
        }
        draft
    }

    fn save_draft(&mut self, cx: &App) {
        // Navigation remains available during sending. Do not queue the old
        // draft behind Send, which clears it only once delivery is accepted.
        if self.busy {
            return;
        }
        self.dirty_since = None;
        let draft = self.current_draft(cx);
        self.draft = draft.clone();
        if self.connected && !self.preview {
            self.command(Command::SaveDraft(draft));
        }
    }

    fn fill_draft(&mut self, draft: Draft, window: &mut Window, cx: &mut Context<Self>) {
        self.from
            .update(cx, |s, cx| s.set_value(draft.from.clone(), window, cx));
        self.to
            .update(cx, |s, cx| s.set_value(draft.to.clone(), window, cx));
        self.subject
            .update(cx, |s, cx| s.set_value(draft.subject.clone(), window, cx));
        self.body
            .update(cx, |s, cx| s.set_value(draft.body.clone(), window, cx));
        self.draft = draft;
        self.dirty_since = None;
    }

    fn show_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview = true;
        self.emails = demo_messages();
        self.rebuild_threads();
        self.view = View::Inbox;
        self.domain = None;
        self.screen = Screen::Mail;
        self.status = "Preview · sample messages".into();
        if let Some(key) = self.thread_key_of("demo-0") {
            self.select(key, window, cx);
        }
    }

    /// Group the mail into conversations.
    ///
    /// A conversation is named after its oldest message, so a message from
    /// earlier in an exchange arriving late can rename the one being read.
    /// The reader follows the message that is open rather than closing itself.
    fn rebuild_threads(&mut self) {
        self.threads = crate::thread::group(&self.emails);
        let anchor = self.open_message.clone().or_else(|| self.selected.clone());
        self.selected = anchor.and_then(|id| {
            self.threads
                .iter()
                .find(|thread| {
                    thread.key == id
                        || thread
                            .messages
                            .iter()
                            .any(|&index| self.emails[index].id == id)
                })
                .map(|thread| thread.key.clone())
        });
        if self.selected.is_none() {
            self.open_message = None;
        }
    }

    /// The conversation a message belongs to.
    ///
    /// Conversations are named rather than numbered everywhere a click can
    /// reach them: a sync between drawing a row and clicking it rebuilds the
    /// list, and a position saved from the old one would open the wrong mail.
    fn thread_key_of(&self, id: &str) -> Option<String> {
        self.threads
            .iter()
            .find(|thread| thread.messages.iter().any(|&i| self.emails[i].id == id))
            .map(|thread| thread.key.clone())
    }

    fn thread_by_key(&self, key: &str) -> Option<usize> {
        self.threads.iter().position(|thread| thread.key == key)
    }

    /// Open a conversation, which reads all of it: it arrived as one exchange
    /// and it is read as one.
    fn select(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_by_key(&key) else {
            return;
        };
        self.selected = Some(key);
        self.open_message = None;
        self.icons_dirty = true;
        self.mark_read(self.conversation(thread), true, cx);
        if let Some(index) = self.open_email() {
            self.show_message(index, window, cx);
        }
    }

    /// Show one message of the conversation being read.
    fn show(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.emails.iter().position(|email| email.id == id) {
            self.show_message(index, window, cx);
        }
    }

    fn show_message(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(email) = self.emails.get(index) else {
            return;
        };
        let (id, folder, body) = (email.id.clone(), email.folder, email.display_body());
        self.open_message = Some(id.clone());
        self.reader
            .update(cx, |state, cx| state.set_value(body, window, cx));
        if self.connected && !self.preview {
            self.command(Command::Open(folder, id));
        }
        cx.notify();
    }

    fn navigate(&mut self, view: View, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.view = view;
        self.screen = Screen::Mail;
        self.selected = None;
        self.open_message = None;
        cx.notify();
    }

    /// Narrow the list to one of your domains, or to all of them with `None`.
    fn filter(&mut self, domain: Option<String>, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.domain = domain;
        self.screen = Screen::Mail;
        self.selected = None;
        self.open_message = None;
        cx.notify();
    }

    /// Mark messages read or unread, on screen and on disk.
    fn mark_read(&mut self, messages: Vec<usize>, read: bool, cx: &mut Context<Self>) {
        let mut changed = Vec::new();
        for index in messages {
            let email = &mut self.emails[index];
            if email.read != read {
                email.read = read;
                changed.push((email.folder, email.id.clone()));
            }
        }
        if !changed.is_empty() && self.connected && !self.preview {
            self.command(Command::SetRead(changed, read));
        }
        cx.notify();
    }

    /// Turn a conversation's read mark over. A conversation that is only
    /// partly read counts as unread, so one press finishes reading it.
    fn toggle_read(&mut self, key: String, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_by_key(&key) else {
            return;
        };
        let messages = self.conversation(thread);
        if messages.iter().all(|&index| self.emails[index].read) {
            self.mark_thread_unread(key, cx);
        } else {
            self.mark_read(messages, true, cx);
        }
    }

    /// Mark a conversation unread and close it. Leaving it open would only
    /// read it again.
    fn mark_thread_unread(&mut self, key: String, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_by_key(&key) else {
            return;
        };
        self.mark_read(self.conversation(thread), false, cx);
        if self.selected.as_deref() == Some(key.as_str()) {
            self.selected = None;
            self.open_message = None;
        }
        cx.notify();
    }

    /// File a whole conversation away, or bring it back.
    fn archive_thread(&mut self, key: String, archived: bool, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_by_key(&key) else {
            return;
        };
        self.archive(self.conversation(thread), archived, cx);
    }

    /// File one message of a conversation away, or bring it back.
    fn archive_message(&mut self, id: String, archived: bool, cx: &mut Context<Self>) {
        if let Some(index) = self.emails.iter().position(|email| email.id == id) {
            self.archive(vec![index], archived, cx);
        }
    }

    /// File messages away, or bring them back.
    ///
    /// This is Receive's own shelf and nothing more: Resend still holds every
    /// message, the Archive view still shows it, and a sync cannot pull it
    /// back into the folder it came from.
    fn archive(&mut self, messages: Vec<usize>, archived: bool, cx: &mut Context<Self>) {
        let mut changed = Vec::new();
        for index in messages {
            let email = &mut self.emails[index];
            if email.archived != archived {
                email.archived = archived;
                changed.push((email.folder, email.id.clone()));
            }
        }
        if changed.is_empty() {
            return;
        }
        let count = changed.len();
        if self.connected && !self.preview {
            self.command(Command::SetArchived(changed, archived));
        }
        // Close the reader only when what it was showing is what just left
        // the view. Filing one conversation away from the list is no reason
        // to shut another one.
        let still_shown = match self.selected_thread() {
            Some(thread) => {
                let messages = self.conversation(thread);
                self.open_message
                    .as_ref()
                    .is_none_or(|id| messages.iter().any(|&index| self.emails[index].id == *id))
            }
            None => false,
        };
        if !still_shown {
            self.selected = None;
            self.open_message = None;
        }
        self.status = match (archived, count) {
            (true, 1) => "Archived on this computer. Resend still has it.".into(),
            (true, count) => {
                format!("Archived {count} messages on this computer. Resend still has them.")
            }
            (false, 1) => "Moved back to its folder.".into(),
            (false, count) => format!("Moved {count} messages back to their folders."),
        };
        cx.notify();
    }

    fn open(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.screen = screen;
        self.icons_dirty = true;
        cx.notify();
    }

    /// Whether a message belongs to the view being shown.
    fn in_view(&self, email: &Email) -> bool {
        match self.view {
            View::Inbox => !email.archived && email.folder == Folder::Inbox,
            View::Sent => !email.archived && email.folder == Folder::Sent,
            View::Archive => email.archived,
        }
    }

    /// The messages of a conversation, oldest first.
    ///
    /// This is the whole exchange across both folders, so a reply sits with
    /// the message it answers. Only what the view itself hides is left out.
    fn conversation(&self, thread: usize) -> Vec<usize> {
        let archived = self.view == View::Archive;
        self.threads
            .get(thread)
            .map(|thread| {
                thread
                    .messages
                    .iter()
                    .copied()
                    .filter(|&index| self.emails[index].archived == archived)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The conversations in view: those with a message in this folder, on the
    /// chosen domain, with something matching the search field.
    fn visible(&self, cx: &App) -> Vec<usize> {
        let query = self.search.read(cx).value().to_lowercase();
        self.threads
            .iter()
            .enumerate()
            .filter(|(_, thread)| {
                thread.messages.iter().any(|&index| {
                    let email = &self.emails[index];
                    self.in_view(email) && self.on_domain(email)
                }) && (query.is_empty()
                    || thread
                        .messages
                        .iter()
                        .any(|&index| matches_query(&self.emails[index], &query)))
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Close a suggestion list shortly after its field lost focus, so a click
    /// on one of the suggestions lands before the list goes away.
    fn close_suggestions(&mut self, cx: &mut Context<Self>) {
        if self
            .closing_suggestions
            .is_some_and(|since| since.elapsed() >= SUGGESTION_GRACE)
        {
            self.closing_suggestions = None;
            self.suggesting = None;
            cx.notify();
        }
    }

    /// Save an address typed in Settings, so it is offered in every draft.
    fn add_contact(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let address = self.contact.read(cx).value().trim().to_string();
        if crate::model::domain_of(&address).is_none() {
            self.error = Some("Enter an address like someone@example.com.".into());
            cx.notify();
            return;
        }
        let known = self
            .contacts
            .iter()
            .any(|saved| saved.eq_ignore_ascii_case(&address));
        if !known {
            self.contacts.push(address);
            self.command(Command::SaveContacts(self.contacts.clone()));
        }
        self.contact
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.error = None;
        cx.notify();
    }

    fn remove_contact(&mut self, address: &str, cx: &mut Context<Self>) {
        self.contacts.retain(|saved| saved != address);
        self.command(Command::SaveContacts(self.contacts.clone()));
        cx.notify();
    }

    /// The domains the account is known to use.
    fn domain_names(&self) -> Vec<String> {
        crate::model::domains_of(&self.emails)
    }

    /// Addresses to offer in a draft field, newest first.
    ///
    /// `To` offers the addresses saved in Settings, then everyone you have
    /// corresponded with. `From` offers only addresses on your own domains,
    /// because Resend will not send from anything else.
    fn address_book(&self, field: Field) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        // One mailbox is one suggestion, however it was written. The form
        // carrying a display name wins, because `Raul <contact@example.com>` is
        // the more useful of the two to put in a header.
        let mut remember = |address: &str| {
            let address = address.trim();
            let Some(mailbox) = crate::model::mailbox_of(address) else {
                return;
            };
            match seen.get(&mailbox) {
                None => {
                    seen.insert(mailbox, found.len());
                    found.push(address.to_string());
                }
                Some(&at) if address.contains('<') && !found[at].contains('<') => {
                    found[at] = address.to_string();
                }
                Some(_) => {}
            }
        };
        if field == Field::To {
            for contact in &self.contacts {
                remember(contact);
            }
        }
        // `self.emails` is newest first, so the most recent correspondence is
        // offered before older addresses.
        for email in &self.emails {
            match (field, email.folder) {
                // Who you have written to, and who has written to you.
                (Field::To, Folder::Sent) => email.to.iter().for_each(|to| remember(to)),
                (Field::To, Folder::Inbox) => remember(&email.from),
                // What you have sent from, and the addresses mail arrives at.
                (Field::From, Folder::Sent) => remember(&email.from),
                (Field::From, Folder::Inbox) => email.to.iter().for_each(|to| remember(to)),
            }
        }
        if field == Field::From {
            let domains = self.domain_names();
            found.retain(|address| {
                crate::model::domain_of(address).is_some_and(|d| domains.contains(&d))
            });
        }
        found
    }

    /// The part of a field the suggestions apply to. `To` takes several
    /// recipients separated by semicolons, so only the one being typed counts.
    fn typing(&self, field: Field, cx: &App) -> String {
        let value = match field {
            Field::From => self.from.read(cx).value().to_string(),
            Field::To => self.to.read(cx).value().to_string(),
        };
        match field {
            Field::From => value,
            Field::To => value.rsplit(';').next().unwrap_or("").to_string(),
        }
    }

    /// The addresses worth showing under a field right now.
    fn suggestions(&self, field: Field, cx: &App) -> Vec<String> {
        let typed = self.typing(field, cx).trim().to_lowercase();
        let already: Vec<String> = match field {
            Field::From => Vec::new(),
            Field::To => self
                .draft_recipients(cx)
                .iter()
                .map(|to| to.to_lowercase())
                .collect(),
        };
        let book = self.address_book(field);
        // A field already holding one of the known addresses offers the others
        // rather than filtering itself down to nothing, so focusing From is how
        // you change sender.
        let settled = book.iter().any(|address| address.to_lowercase() == typed);
        book.iter()
            .filter(|address| {
                let lower = address.to_lowercase();
                lower != typed && !already.contains(&lower) && (settled || lower.contains(&typed))
            })
            .take(SUGGESTIONS)
            .cloned()
            .collect()
    }

    /// The recipients already committed in the To field, ignoring the one being
    /// typed after the last semicolon.
    fn draft_recipients(&self, cx: &App) -> Vec<String> {
        let value = self.to.read(cx).value().to_string();
        let (committed, _) = value.rsplit_once(';').unwrap_or(("", ""));
        committed
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }

    /// Put a chosen address into its field, replacing what was being typed.
    ///
    /// The field keeps focus afterwards: pressing a suggestion takes focus off
    /// it, and typing should carry on where it left off. For To that also means
    /// the list reopens on the empty segment, ready for the next recipient.
    fn accept(&mut self, field: Field, address: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (state, value) = match field {
            Field::From => (&self.from, address.to_string()),
            Field::To => {
                let mut recipients = self.draft_recipients(cx);
                recipients.push(address.to_string());
                (&self.to, format!("{}; ", recipients.join("; ")))
            }
        };
        state.clone().update(cx, |state, cx| {
            state.set_value(value, window, cx);
            state.focus(window, cx);
        });
        self.suggesting = None;
        self.closing_suggestions = None;
        self.highlighted = 0;
        self.dirty_since = Some(Instant::now());
        cx.notify();
    }

    /// Move the arrow-key highlight through a field's suggestions, wrapping at
    /// either end.
    fn highlight(&mut self, by: isize, count: usize) {
        if count == 0 {
            return;
        }
        let at = self.highlighted.min(count - 1) as isize;
        self.highlighted = (at + by).rem_euclid(count as isize) as usize;
    }

    /// The suggestion the arrow keys are on, clamped in case the list shrank
    /// under them.
    fn highlighted_of(&self, options: &[String]) -> Option<String> {
        options
            .get(self.highlighted.min(options.len().saturating_sub(1)))
            .cloned()
    }

    fn on_domain(&self, email: &Email) -> bool {
        self.domain
            .as_ref()
            .is_none_or(|domain| email.domains().iter().any(|found| found == domain))
    }

    /// Unread received mail, on the chosen domain when one is chosen.
    fn unread(&self) -> usize {
        self.emails
            .iter()
            .filter(|e| e.folder == Folder::Inbox && !e.read && !e.archived && self.on_domain(e))
            .count()
    }

    /// Every domain mail has arrived at or been sent from, in alphabetical
    /// order, each with the number of messages still unread in its inbox.
    fn domains(&self) -> Vec<(String, usize)> {
        let mut domains: BTreeMap<String, usize> = BTreeMap::new();
        for email in &self.emails {
            let unread =
                usize::from(email.folder == Folder::Inbox && !email.read && !email.archived);
            for domain in email.domains() {
                *domains.entry(domain).or_default() += unread;
            }
        }
        domains.into_iter().collect()
    }

    /// The conversation being read, if it is still in view.
    fn selected_thread(&self) -> Option<usize> {
        let key = self.selected.as_ref()?;
        let thread = self.threads.iter().position(|thread| &thread.key == key)?;
        (!self.conversation(thread).is_empty()).then_some(thread)
    }

    /// The message shown open inside it: the one chosen, or the most recent.
    fn open_email(&self) -> Option<usize> {
        let messages = self.conversation(self.selected_thread()?);
        self.open_message
            .as_ref()
            .and_then(|id| {
                messages
                    .iter()
                    .copied()
                    .find(|&index| self.emails[index].id == *id)
            })
            .or_else(|| messages.last().copied())
    }

    pub(super) fn reading(&self) -> Option<&Email> {
        self.emails.get(self.open_email()?)
    }

    fn reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(email) = self.reading().cloned() else {
            return;
        };
        if !self.draft.body.trim().is_empty() || !self.draft.to.trim().is_empty() {
            self.error = Some(
                "You have an unfinished draft. Open Compose to send it or clear it before replying."
                    .into(),
            );
            return;
        }
        let from = self.from.read(cx).value().to_string();
        self.fill_draft(
            Draft {
                from,
                to: email.reply_recipient(),
                subject: email.reply_subject(),
                in_reply_to: email.message_id.clone(),
                references: email.references(),
                ..Default::default()
            },
            window,
            cx,
        );
        self.screen = Screen::Compose;
        self.save_draft(cx);
        cx.notify();
    }

    /// The window's own title bar, so the frame is as black as the rest.
    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new()
            .on_close_window(cx.listener(|this, _, window, cx| {
                if this.request_close(cx) {
                    window.remove_window();
                }
            }))
            .child(
                h_flex()
                    .h_full()
                    .items_center()
                    .gap_2()
                    .child(mark(px(15.)))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Receive"),
                    ),
            )
    }

    /// The preview and error notices that sit above the open pane.
    fn banners(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_shrink_0()
            .when(self.preview, |this| {
                this.child(
                    h_flex()
                        .h(px(30.))
                        .px_5()
                        .gap_2()
                        .items_center()
                        .border_b_1()
                        .border_color(theme::line(cx))
                        .bg(theme::surface(cx))
                        .text_xs()
                        .text_color(theme::muted(cx))
                        .child(Icon::new(IconName::Eye).xsmall())
                        .child(
                            "Sample messages. Connect your account in Settings to see your own.",
                        ),
                )
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    h_flex()
                        .px_5()
                        .py_3()
                        .gap_3()
                        .items_center()
                        .border_b_1()
                        .border_color(cx.theme().danger.opacity(0.35))
                        .bg(cx.theme().danger.opacity(0.14))
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .small()
                                .text_color(cx.theme().danger),
                        )
                        .child(div().flex_1().text_sm().child(error))
                        .child(
                            Button::new("dismiss-error")
                                .ghost()
                                .xsmall()
                                .icon(IconName::X)
                                .tooltip("Dismiss")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
    }

    /// Whether Receive is showing your mail, someone else's samples, or nothing.
    ///
    /// It sits at the head of the status bar, where the rest of what the worker
    /// is doing is already reported.
    fn connection(&self, cx: &App) -> impl IntoElement {
        let (color, label) = if self.preview {
            (theme::muted(cx), "Sample inbox · nothing here is real")
        } else if self.connected {
            (
                cx.theme().success,
                "Resend connected · saved on this computer",
            )
        } else {
            (
                theme::muted(cx),
                "Not connected · add an account in Settings",
            )
        };
        h_flex()
            .id("connection")
            .flex_shrink_0()
            .items_center()
            .justify_center()
            // A hit area the pointer can actually find around a 6px dot.
            .size(px(14.))
            .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
            .child(div().size(px(6.)).rounded_full().bg(color))
    }

    fn status_bar(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .h(px(28.))
            .flex_shrink_0()
            .px_4()
            .gap_2()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(theme::line(cx))
            .text_xs()
            .text_color(theme::muted(cx))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .min_w_0()
                    .child(self.connection(cx))
                    .when(self.busy, |this| {
                        this.child(Spinner::new().xsmall().color(theme::muted(cx)))
                    })
                    .child(div().truncate().child(self.status.clone())),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .child(format!("Receive {}", env!("CARGO_PKG_VERSION"))),
            )
    }

    fn advance_smoke_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.smoke_test {
            return;
        }
        self.smoke_ticks += 1;
        match self.smoke_ticks {
            10 => {
                self.view = View::Sent;
                if let Some(key) = self.thread_key_of("demo-bounced") {
                    self.select(key, window, cx);
                }
            }
            20 => {
                self.screen = Screen::Compose;
                cx.notify();
            }
            30 => {
                self.screen = Screen::Settings;
                cx.notify();
            }
            40 => {
                if self.smoke_frames == 15 {
                    self.check_draft_transitions(window, cx);
                    eprintln!(
                        "UI smoke test passed: inbox, sent statuses, composer, settings, and draft transitions."
                    );
                } else {
                    eprintln!(
                        "UI smoke test failed: missing rendered screens ({})",
                        self.smoke_frames
                    );
                    std::process::exit(1);
                }
                cx.quit();
            }
            _ => {}
        }
    }

    /// Exercise real UI state transitions with a local command channel. No
    /// credentials, API requests, or sent messages are involved.
    fn check_draft_transitions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (commands, incoming) = std::sync::mpsc::channel();
        let (outgoing, events) = std::sync::mpsc::channel();
        self.worker = Worker { commands, events };
        self.preview = false;
        self.connected = true;
        self.fill_draft(
            Draft {
                to: "sample@example.com".into(),
                body: "Pending send".into(),
                idempotency_key: "smoke-send".into(),
                ..Default::default()
            },
            window,
            cx,
        );
        self.busy = true;
        self.open(Screen::Mail, cx);
        assert!(
            incoming.try_recv().is_err(),
            "Navigation must not re-save an in-flight draft"
        );
        assert!(!self.request_close(cx));
        assert!(matches!(incoming.try_recv(), Ok(Command::FlushDraft(None))));
        assert!(!self.request_close(cx));
        assert!(incoming.try_recv().is_err(), "Closing is queued only once");
        outgoing.send(Event::Sent).unwrap();
        self.handle_events(window, cx);
        assert!(self.current_draft(cx).body.is_empty());

        self.busy = false;
        self.closing = false;
        self.body.update(cx, |state, cx| {
            state.set_value("Last unsaved edit", window, cx)
        });
        assert!(!self.request_close(cx));
        let Ok(Command::FlushDraft(Some(draft))) = incoming.try_recv() else {
            panic!("Closing must flush the current field values before exiting");
        };
        assert_eq!(draft.body, "Last unsaved edit");
    }
}

impl Render for Receive {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.smoke_test {
            self.smoke_frames |= match self.screen {
                Screen::Mail if self.view == View::Sent => 8,
                Screen::Mail => 1,
                Screen::Compose => 2,
                Screen::Settings => 4,
            };
        }
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .child(self.title_bar(cx))
            .child(
                h_flex().flex_1().min_h_0().child(self.sidebar(cx)).child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(self.banners(cx))
                        .child(
                            h_flex().flex_1().min_h_0().child(match self.screen {
                                Screen::Mail => h_flex()
                                    .size_full()
                                    .child(self.message_list(cx))
                                    .child(self.reader(cx))
                                    .into_any_element(),
                                Screen::Compose => self.compose(cx).into_any_element(),
                                Screen::Settings => self.settings(cx).into_any_element(),
                            }),
                        ),
                ),
            )
            .child(self.status_bar(cx))
    }
}

/// Whether a message answers what was typed in the search field.
fn matches_query(email: &Email, query: &str) -> bool {
    format!(
        "{} {} {} {} {}",
        email.from,
        email.to.join(" "),
        email.subject,
        email.text.as_deref().unwrap_or(""),
        if email.folder == Folder::Sent {
            email.delivery_status().label()
        } else {
            ""
        }
    )
    .to_lowercase()
    .contains(query)
}

/// What an address shows when its domain has no icon to show.
fn glyph(color: Hsla) -> Icon {
    Icon::new(IconName::AtSign).xsmall().text_color(color)
}

/// The mail glyph from the application icon.
pub fn mark(size: Pixels) -> Icon {
    Icon::default().path(assets::MARK).size(size)
}

/// The name a message is filed under, without its address.
pub fn display_name(address: &str) -> String {
    let name = address
        .split('<')
        .next()
        .unwrap_or(address)
        .trim()
        .trim_matches('"')
        .trim();
    if name.is_empty() {
        address.trim().into()
    } else {
        name.into()
    }
}

/// The letter shown on a correspondent's avatar.
pub fn initial(address: &str) -> String {
    display_name(address)
        .chars()
        .find(|c| c.is_alphanumeric())
        .unwrap_or('?')
        .to_uppercase()
        .to_string()
}

pub fn short_date(value: &str) -> String {
    let Some(date) = crate::model::parse_date(value) else {
        return String::new();
    };
    let date = date.with_timezone(&chrono::Local);
    let now = chrono::Local::now();
    if date.date_naive() == now.date_naive() {
        date.format("%H:%M").to_string()
    } else if date.year() == now.year() {
        date.format("%b %d").to_string()
    } else {
        date.format("%b %Y").to_string()
    }
}

pub fn long_date(value: &str) -> String {
    crate::model::parse_date(value)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%b %d, %Y at %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}
