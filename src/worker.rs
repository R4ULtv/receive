use crate::{
    api::{Api, http_client},
    auth::{Credentials, oauth_login},
    model::{DeliveryStatus, Draft, Email, Folder},
    store::Store,
};
use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

pub enum Command {
    ConnectKey(String),
    ConnectOAuth,
    Disconnect,
    Sync,
    Open(Folder, String),
    SaveDraft(Draft),
    FlushDraft(Option<Draft>),
    SaveContacts(Vec<String>),
    Send(Draft),
    /// Mark a conversation, or one message of it, read or unread.
    SetRead(Vec<(Folder, String)>, bool),
    /// File a conversation, or one message of it, away — or bring it back.
    SetArchived(Vec<(Folder, String)>, bool),
}

pub enum Event {
    Connected {
        emails: Vec<Email>,
        draft: Draft,
        contacts: Vec<String>,
    },
    Disconnected,
    Snapshot(Vec<Email>),
    Updated(Box<Email>),
    Sent,
    Status(String),
    Error(String),
    Busy(bool),
    DraftFlushed,
}

pub struct Worker {
    pub commands: Sender<Command>,
    pub events: Receiver<Event>,
}

impl Worker {
    pub fn start(data_dir: PathBuf, load_saved_account: bool) -> Self {
        let (commands, incoming) = mpsc::channel();
        let (outgoing, events) = mpsc::channel();
        std::thread::spawn(move || {
            let result = run(data_dir, load_saved_account, incoming, &outgoing);
            if let Err(error) = result {
                let _ = outgoing.send(Event::Error(format!("{error:#}")));
            }
        });
        Self { commands, events }
    }
}

fn emit(events: &Sender<Event>, event: Event) {
    let _ = events.send(event);
}

fn run(
    data_dir: PathBuf,
    load_saved_account: bool,
    incoming: Receiver<Command>,
    events: &Sender<Event>,
) -> Result<()> {
    std::fs::create_dir_all(&data_dir)?;
    let store = Store::open(&data_dir.join("mail.sqlite3"))?;
    let mut api = match if load_saved_account {
        Credentials::load()
    } else {
        Ok(None)
    } {
        Ok(Some(credentials)) => Some(Api::new(credentials)?),
        Ok(None) => None,
        Err(error) => {
            emit(events, Event::Error(format!("{error:#}")));
            None
        }
    };
    if let Some(api) = &api {
        connected(api, &store, events)?;
    }
    let mut next_sync = Instant::now();
    let mut next_archive = Instant::now();
    let mut failures = 0_u32;
    loop {
        match incoming.recv_timeout(Duration::from_millis(200)) {
            Ok(command) => {
                let reports_busy = matches!(
                    &command,
                    Command::ConnectKey(_)
                        | Command::ConnectOAuth
                        | Command::Send(_)
                        | Command::FlushDraft(_)
                );
                let result: Result<()> = (|| {
                    match command {
                        Command::ConnectKey(key) => {
                            emit(events, Event::Busy(true));
                            emit(events, Event::Status("Connecting to Resend…".into()));
                            let mut candidate = Api::new(Credentials::api_key(key.trim().into()))?;
                            candidate
                                .list(Folder::Inbox, None)
                                .context("Use a Full access Resend API key to read mail")?;
                            candidate.credentials.save()?;
                            connected(&candidate, &store, events)?;
                            api = Some(candidate);
                            next_sync = Instant::now();
                            failures = 0;
                        }
                        Command::ConnectOAuth => {
                            emit(events, Event::Busy(true));
                            emit(
                                events,
                                Event::Status("Finish connecting in your browser…".into()),
                            );
                            let client = http_client()?;
                            let mut credentials = oauth_login(&client, |url| {
                                webbrowser::open(url)?;
                                Ok(())
                            })?;
                            credentials.save()?;
                            let candidate = Api::new(credentials)?;
                            connected(&candidate, &store, events)?;
                            api = Some(candidate);
                            next_sync = Instant::now();
                            failures = 0;
                        }
                        Command::Disconnect => {
                            Credentials::forget()?;
                            api = None;
                            emit(events, Event::Disconnected);
                            emit(
                                events,
                                Event::Status(
                                    "Disconnected. Your local archive is kept on this computer."
                                        .into(),
                                ),
                            );
                        }
                        Command::Sync => {
                            next_sync = Instant::now();
                        }
                        Command::Open(folder, id) => {
                            if let Some(api) = &mut api {
                                let profile = api.credentials.profile.clone();
                                store.set_read(&profile, &[(folder, id.clone())], true)?;
                                if let Some(mut email) = store.get(&profile, folder, &id)? {
                                    if !email.body_loaded {
                                        email = api.email(folder, &id)?;
                                        email.read = true;
                                        store.put(&profile, &email)?;
                                        email = store
                                            .get(&profile, folder, &id)?
                                            .context("Saved message is missing")?;
                                    }
                                    emit(events, Event::Updated(Box::new(email)));
                                }
                            }
                        }
                        Command::SaveDraft(draft) => {
                            if let Some(api) = &api {
                                store.save_draft(&api.credentials.profile, &draft)?;
                            }
                        }
                        Command::FlushDraft(draft) => {
                            if let (Some(api), Some(draft)) = (&api, draft) {
                                store.save_draft(&api.credentials.profile, &draft)?;
                            }
                            emit(events, Event::DraftFlushed);
                            return Ok(());
                        }
                        Command::SetRead(messages, read) => {
                            if let Some(api) = &api {
                                store.set_read(&api.credentials.profile, &messages, read)?;
                            }
                        }
                        Command::SetArchived(messages, archived) => {
                            if let Some(api) = &api {
                                store.set_archived(&api.credentials.profile, &messages, archived)?;
                            }
                        }
                        Command::SaveContacts(contacts) => {
                            if let Some(api) = &api {
                                store.save_contacts(&api.credentials.profile, &contacts)?;
                            }
                        }
                        Command::Send(draft) => {
                            let api = api
                                .as_mut()
                                .context("Connect your Resend account before sending")?;
                            emit(events, Event::Busy(true));
                            draft.validate()?;
                            // Persist the exact retry identity BEFORE attempting delivery.
                            store.save_draft(&api.credentials.profile, &draft)?;
                            let id = api.send(&draft)?;
                            // Resend's sent mail carries no headers back, so
                            // the only record that this answers something is
                            // the one written here. Without it the reply can
                            // never be threaded with the message it answers.
                            let mut headers = BTreeMap::new();
                            if let Some(answering) = &draft.in_reply_to {
                                headers.insert("In-Reply-To".into(), answering.clone());
                                headers.insert("References".into(), draft.references.clone());
                            }
                            let sent = Email {
                                id,
                                from: draft.from.clone(),
                                to: draft.recipients(),
                                subject: draft.subject.clone(),
                                text: Some(draft.body.clone()),
                                created_at: chrono::Utc::now().to_rfc3339(),
                                folder: Folder::Sent,
                                read: true,
                                body_loaded: true,
                                last_event: Some(DeliveryStatus::Submitted),
                                headers,
                                ..Default::default()
                            };
                            store.put(&api.credentials.profile, &sent)?;
                            store.save_draft(
                                &api.credentials.profile,
                                &Draft {
                                    from: draft.from,
                                    ..Default::default()
                                },
                            )?;
                            emit(events, Event::Updated(Box::new(sent)));
                            emit(events, Event::Sent);
                            emit(events, Event::Status("Sent to Resend for delivery.".into()));
                            next_sync = Instant::now() + Duration::from_secs(3);
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    emit(events, Event::Error(format!("{error:#}")));
                }
                if reports_busy {
                    emit(events, Event::Busy(false));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let Some(api) = &mut api else {
            continue;
        };
        if Instant::now() >= next_sync {
            emit(events, Event::Status("Checking for new mail…".into()));
            match sync(api, &store, events) {
                Ok(errors) => {
                    // A failure in one folder must not suppress the other folder
                    // or prevent successfully listed messages from being archived.
                    failures = if errors.len() == 2 {
                        (failures + 1).min(4)
                    } else {
                        0
                    };
                    if errors.is_empty() {
                        emit(
                            events,
                            Event::Status(format!(
                                "Updated {} · checks every 60s",
                                chrono::Local::now().format("%H:%M")
                            )),
                        );
                    } else {
                        emit(
                            events,
                            Event::Status(
                                "Showing available mail · retrying failed folders automatically"
                                    .into(),
                            ),
                        );
                        emit(events, Event::Error(errors.join("\n")));
                    }
                }
                Err(error) => {
                    failures = (failures + 1).min(4);
                    emit(events, Event::Error(format!("Sync paused: {error:#}")));
                }
            }
            next_sync = Instant::now() + Duration::from_secs(60 * 2_u64.pow(failures));
        }
        if failures == 0
            && Instant::now() >= next_archive
            && let Some(email) = store.pending(&api.credentials.profile)?
        {
            match api.email(email.folder, &email.id) {
                Ok(mut complete) => {
                    // Update the archived body without resetting the local read flag.
                    complete.read = store
                        .get(&api.credentials.profile, email.folder, &email.id)?
                        .is_some_and(|e| e.read);
                    store.put(&api.credentials.profile, &complete)?;
                    let complete = store
                        .get(&api.credentials.profile, email.folder, &email.id)?
                        .context("Saved message is missing")?;
                    emit(events, Event::Updated(Box::new(complete)));
                }
                Err(error) => {
                    let unavailable = crate::api::body_unavailable(&error);
                    store.defer_body(&api.credentials.profile, &email, unavailable)?;
                    emit(
                        events,
                        Event::Error(format!("Could not archive a message: {error:#}")),
                    );
                    // Missing/expired bodies cannot become available by
                    // polling. Move on immediately instead of starving newer
                    // mail behind a repeating group of expired messages.
                    if !unavailable {
                        next_archive = Instant::now() + Duration::from_secs(60);
                    }
                }
            }
        }
    }
}

fn connected(api: &Api, store: &Store, events: &Sender<Event>) -> Result<()> {
    emit(
        events,
        Event::Connected {
            emails: store.list(&api.credentials.profile)?,
            draft: store.draft(&api.credentials.profile)?,
            contacts: store.contacts(&api.credentials.profile)?,
        },
    );
    Ok(())
}

fn sync(api: &mut Api, store: &Store, events: &Sender<Event>) -> Result<Vec<String>> {
    let profile = api.credentials.profile.clone();
    sync_mailboxes(store, &profile, events, |folder, after| {
        api.list(folder, after)
    })
}

fn sync_mailboxes(
    store: &Store,
    profile: &str,
    events: &Sender<Event>,
    mut fetch: impl FnMut(Folder, Option<&str>) -> Result<crate::api::Page>,
) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    for folder in [Folder::Inbox, Folder::Sent] {
        if let Err(error) = sync_folder(store, profile, folder, |after| fetch(folder, after)) {
            errors.push(format!("{} sync failed: {error:#}", folder.name()));
        }
        // Publish even partially saved pages, independently of the other folder.
        emit(events, Event::Snapshot(store.list(profile)?));
    }
    Ok(errors)
}

fn sync_folder(
    store: &Store,
    profile: &str,
    folder: Folder,
    mut fetch: impl FnMut(Option<&str>) -> Result<crate::api::Page>,
) -> Result<()> {
    let setting = format!("checkpoint-{}", folder.name());
    let checkpoint = store.setting(profile, &setting)?;
    let mut after: Option<String> = None;
    let mut newest = None;
    let mut visited = std::collections::HashSet::new();
    loop {
        let page = fetch(after.as_deref())?;
        if newest.is_none() {
            newest = page.data.first().map(|e| e.id.clone());
        }
        // Sent statuses can change well after a message was first seen. Read
        // every available page on each poll, including messages beyond the old
        // checkpoint. Inbox can still stop at the first already-seen message.
        let hit_checkpoint = folder == Folder::Inbox
            && page
                .data
                .iter()
                .any(|email| Some(&email.id) == checkpoint.as_ref());
        store.put_page(profile, &page.data)?;
        if hit_checkpoint || !page.has_more {
            break;
        }
        let last = page
            .data
            .last()
            .context("Resend returned an empty page with has_more=true")?;
        anyhow::ensure!(
            visited.insert(last.id.clone()),
            "Resend repeated a pagination cursor"
        );
        after = Some(last.id.clone());
    }
    // Advance only after every intervening page has been saved successfully.
    if let Some(id) = newest {
        store.set_setting(profile, &setting, &id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Page;

    #[test]
    fn sent_poll_refreshes_status_beyond_checkpoint_even_without_new_mail() -> Result<()> {
        let store = Store::open(std::path::Path::new(":memory:"))?;
        store.set_setting("a", "checkpoint-Sent", "newest")?;
        store.put(
            "a",
            &Email {
                id: "older".into(),
                folder: Folder::Sent,
                body_loaded: true,
                read: true,
                text: Some("Archived body".into()),
                last_event: Some(DeliveryStatus::Sent),
                ..Default::default()
            },
        )?;
        let (events, receiver) = mpsc::channel();
        let mut sent_calls = 0;
        let errors = sync_mailboxes(&store, "a", &events, |folder, cursor| {
            if folder == Folder::Inbox {
                return Ok(page(&[], false));
            }
            sent_calls += 1;
            let (id, status, has_more) = match cursor {
                None => ("newest", DeliveryStatus::Delivered, true),
                Some("newest") => ("middle", DeliveryStatus::DeliveryDelayed, true),
                Some("middle") => ("older", DeliveryStatus::Bounced, false),
                _ => panic!("Unexpected cursor"),
            };
            Ok(Page {
                data: vec![Email {
                    id: id.into(),
                    folder,
                    last_event: Some(status),
                    status_checked_at: Some("2026-09-19T10:00:00Z".into()),
                    ..Default::default()
                }],
                has_more,
            })
        })?;
        assert!(errors.is_empty());
        assert_eq!(sent_calls, 3);
        let snapshot = receiver
            .try_iter()
            .filter_map(|event| {
                if let Event::Snapshot(emails) = event {
                    Some(emails)
                } else {
                    None
                }
            })
            .last()
            .unwrap();
        let older = snapshot.iter().find(|email| email.id == "older").unwrap();
        assert_eq!(older.last_event, Some(DeliveryStatus::Bounced));
        assert_eq!(older.text.as_deref(), Some("Archived body"));
        assert!(older.read && older.body_loaded);
        Ok(())
    }

    #[test]
    fn failed_status_poll_keeps_last_known_status_and_check_time() -> Result<()> {
        let store = Store::open(std::path::Path::new(":memory:"))?;
        store.set_setting("a", "checkpoint-Sent", "older")?;
        store.put(
            "a",
            &Email {
                id: "older".into(),
                folder: Folder::Sent,
                last_event: Some(DeliveryStatus::Sent),
                status_checked_at: Some("2026-09-18T10:00:00Z".into()),
                ..Default::default()
            },
        )?;
        assert!(
            sync_folder(&store, "a", Folder::Sent, |cursor| {
                if cursor.is_some() {
                    anyhow::bail!("offline");
                }
                let mut first = page(&["newest"], true);
                first.data[0].folder = Folder::Sent;
                Ok(first)
            })
            .is_err()
        );
        let older = store.get("a", Folder::Sent, "older")?.unwrap();
        assert_eq!(older.last_event, Some(DeliveryStatus::Sent));
        assert_eq!(
            older.status_checked_at.as_deref(),
            Some("2026-09-18T10:00:00Z")
        );
        assert_eq!(
            store.setting("a", "checkpoint-Sent")?.as_deref(),
            Some("older")
        );
        Ok(())
    }

    #[test]
    fn a_broken_folder_does_not_hide_successful_mail_or_skip_the_other_folder() -> Result<()> {
        for broken in [Folder::Inbox, Folder::Sent] {
            let store = Store::open(std::path::Path::new(":memory:"))?;
            let (events, receiver) = mpsc::channel();
            let mut visited = Vec::new();
            let errors = sync_mailboxes(&store, "a", &events, |folder, _| {
                visited.push(folder);
                if folder == broken {
                    anyhow::bail!("simulated response failure");
                }
                Ok(Page {
                    data: vec![Email {
                        id: "ok".into(),
                        folder,
                        ..Default::default()
                    }],
                    has_more: false,
                })
            })?;
            assert_eq!(visited, vec![Folder::Inbox, Folder::Sent]);
            assert_eq!(errors.len(), 1);
            assert!(errors[0].starts_with(broken.name()));
            assert!(
                receiver
                    .try_iter()
                    .any(|event| matches!(event, Event::Snapshot(emails) if emails.len() == 1))
            );
            assert!(store.pending("a")?.is_some());
        }
        Ok(())
    }
    fn page(ids: &[&str], has_more: bool) -> Page {
        Page {
            data: ids
                .iter()
                .map(|id| Email {
                    id: (*id).into(),
                    ..Default::default()
                })
                .collect(),
            has_more,
        }
    }
    #[test]
    fn interrupted_initial_sync_does_not_skip_older_pages_on_retry() -> Result<()> {
        let store = Store::open(std::path::Path::new(":memory:"))?;
        let first = sync_folder(&store, "account", Folder::Inbox, |cursor| {
            if cursor.is_none() {
                Ok(page(&["new", "middle"], true))
            } else {
                anyhow::bail!("simulated offline")
            }
        });
        assert!(first.is_err());
        assert_eq!(store.setting("account", "checkpoint-Inbox")?, None);
        let mut cursors = Vec::new();
        sync_folder(&store, "account", Folder::Inbox, |cursor| {
            cursors.push(cursor.map(str::to_owned));
            if cursor.is_none() {
                Ok(page(&["new", "middle"], true))
            } else {
                Ok(page(&["old"], false))
            }
        })?;
        assert_eq!(cursors, vec![None, Some("middle".into())]);
        assert_eq!(store.list("account")?.len(), 3);
        assert_eq!(
            store.setting("account", "checkpoint-Inbox")?.as_deref(),
            Some("new")
        );
        Ok(())
    }
    #[test]
    fn polling_fetches_all_pages_of_new_mail_and_stops_at_checkpoint() -> Result<()> {
        let store = Store::open(std::path::Path::new(":memory:"))?;
        store.set_setting("a", "checkpoint-Inbox", "old")?;
        let mut calls = 0;
        sync_folder(&store, "a", Folder::Inbox, |cursor| {
            calls += 1;
            if cursor.is_none() {
                Ok(page(&["newest", "newer"], true))
            } else {
                Ok(page(&["new", "old"], true))
            }
        })?;
        assert_eq!(calls, 2);
        assert_eq!(store.list("a")?.len(), 4);
        Ok(())
    }
    #[test]
    fn repeated_cursor_fails_without_advancing_checkpoint() -> Result<()> {
        let store = Store::open(std::path::Path::new(":memory:"))?;
        assert!(sync_folder(&store, "a", Folder::Inbox, |_| Ok(page(&["same"], true))).is_err());
        assert_eq!(store.setting("a", "checkpoint-Inbox")?, None);
        Ok(())
    }
}
