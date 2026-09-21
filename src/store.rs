use crate::model::{Draft, Email, Folder};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub struct Store {
    db: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS messages (
                profile TEXT NOT NULL, folder TEXT NOT NULL, id TEXT NOT NULL,
                created_at TEXT NOT NULL, data TEXT NOT NULL, is_read INTEGER NOT NULL DEFAULT 0,
                body_loaded INTEGER NOT NULL DEFAULT 0, retry_after INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(profile, folder, id));
            CREATE INDEX IF NOT EXISTS message_date ON messages(profile, created_at DESC);
            CREATE INDEX IF NOT EXISTS pending_body ON messages(profile, created_at) WHERE body_loaded=0;
            CREATE TABLE IF NOT EXISTS settings (profile TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(profile, name));")?;
        Ok(Self { db })
    }
    pub fn put(&self, profile: &str, email: &Email) -> Result<()> {
        // Keep archived content/read state, but always merge newly observed delivery
        // status. Missing/null status must not erase the last known result.
        self.db.prepare_cached("INSERT INTO messages(profile,folder,id,created_at,data,is_read,body_loaded)
            VALUES (?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(profile,folder,id) DO UPDATE SET
                data=json_set(
                    CASE WHEN excluded.body_loaded=1 OR messages.body_loaded=0 THEN excluded.data ELSE messages.data END,
                    '$.last_event', COALESCE(json_extract(excluded.data, '$.last_event'), json_extract(messages.data, '$.last_event')),
                    '$.status_checked_at', CASE WHEN json_extract(excluded.data, '$.last_event') IS NOT NULL
                        THEN json_extract(excluded.data, '$.status_checked_at')
                        ELSE json_extract(messages.data, '$.status_checked_at') END),
                body_loaded=MAX(messages.body_loaded,excluded.body_loaded)")?
            .execute(params![profile, email.folder.name(), email.id, email.created_at, serde_json::to_string(email)?, email.read, email.body_loaded])?;
        Ok(())
    }
    pub fn put_page(&self, profile: &str, emails: &[Email]) -> Result<()> {
        // One commit per API page instead of one disk transaction per message.
        let transaction = self.db.unchecked_transaction()?;
        for email in emails {
            self.put(profile, email)?;
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn list(&self, profile: &str) -> Result<Vec<Email>> {
        let mut stmt = self.db.prepare("SELECT data,is_read,body_loaded FROM messages WHERE profile=?1 ORDER BY created_at DESC")?;
        let rows = stmt.query_map([profile], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?;
        let mut emails = rows
            .map(|row| {
                let (data, read, loaded) = row?;
                let mut email: Email = serde_json::from_str(&data)?;
                email.read = read;
                email.body_loaded = loaded;
                Ok(email)
            })
            .collect::<Result<Vec<_>>>()?;
        crate::model::sort_emails(&mut emails);
        Ok(emails)
    }
    pub fn get(&self, profile: &str, folder: Folder, id: &str) -> Result<Option<Email>> {
        let row = self.db.query_row("SELECT data,is_read,body_loaded FROM messages WHERE profile=?1 AND folder=?2 AND id=?3", params![profile, folder.name(), id], |row| Ok((row.get::<_,String>(0)?, row.get::<_,bool>(1)?, row.get::<_,bool>(2)?))).optional()?;
        row.map(|(data, read, loaded)| {
            let mut email: Email = serde_json::from_str(&data)?;
            email.read = read;
            email.body_loaded = loaded;
            Ok(email)
        })
        .transpose()
    }
    pub fn mark_read(&self, profile: &str, folder: Folder, id: &str) -> Result<()> {
        self.db.execute(
            "UPDATE messages SET is_read=1 WHERE profile=?1 AND folder=?2 AND id=?3",
            params![profile, folder.name(), id],
        )?;
        Ok(())
    }
    pub fn pending(&self, profile: &str) -> Result<Option<Email>> {
        let data: Option<String> = self.db.query_row("SELECT data FROM messages WHERE profile=?1 AND body_loaded=0 AND retry_after <= unixepoch() ORDER BY created_at ASC LIMIT 1", [profile], |row| row.get(0)).optional()?;
        data.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
    }
    pub fn defer_body(&self, profile: &str, email: &Email, unavailable: bool) -> Result<()> {
        self.db.execute("UPDATE messages SET retry_after=CASE WHEN ?4 THEN 9223372036854775807 ELSE unixepoch()+300 END WHERE profile=?1 AND folder=?2 AND id=?3", params![profile, email.folder.name(), email.id, unavailable])?;
        Ok(())
    }
    pub fn setting(&self, profile: &str, name: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row(
                "SELECT value FROM settings WHERE profile=?1 AND name=?2",
                params![profile, name],
                |row| row.get(0),
            )
            .optional()?)
    }
    pub fn set_setting(&self, profile: &str, name: &str, value: &str) -> Result<()> {
        self.db.execute("INSERT INTO settings(profile,name,value) VALUES (?1,?2,?3) ON CONFLICT(profile,name) DO UPDATE SET value=excluded.value", params![profile,name,value])?;
        Ok(())
    }
    pub fn draft(&self, profile: &str) -> Result<Draft> {
        self.setting(profile, "draft")?
            .map(|s| Ok(serde_json::from_str(&s)?))
            .unwrap_or(Ok(Draft::default()))
    }
    pub fn save_draft(&self, profile: &str, draft: &Draft) -> Result<()> {
        self.set_setting(profile, "draft", &serde_json::to_string(draft)?)
    }
    /// The addresses saved by hand in Settings, offered alongside the ones
    /// Receive works out from your mail.
    pub fn contacts(&self, profile: &str) -> Result<Vec<String>> {
        self.setting(profile, "contacts")?
            .map(|s| Ok(serde_json::from_str(&s)?))
            .unwrap_or(Ok(Vec::new()))
    }
    pub fn save_contacts(&self, profile: &str, contacts: &[String]) -> Result<()> {
        self.set_setting(profile, "contacts", &serde_json::to_string(contacts)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeliveryStatus;

    #[test]
    fn a_failed_page_rolls_back_every_message() -> Result<()> {
        let store = Store::open(Path::new(":memory:"))?;
        store.db.execute_batch("CREATE TRIGGER reject_message BEFORE INSERT ON messages WHEN NEW.id='bad' BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;")?;
        let page = [
            Email {
                id: "good".into(),
                ..Default::default()
            },
            Email {
                id: "bad".into(),
                ..Default::default()
            },
        ];
        assert!(store.put_page("account", &page).is_err());
        assert!(store.list("account")?.is_empty());
        Ok(())
    }

    #[test]
    fn unavailable_old_bodies_do_not_starve_downloads() -> Result<()> {
        let store = Store::open(Path::new(":memory:"))?;
        for index in 0..6 {
            let email = Email {
                id: index.to_string(),
                created_at: format!("2026-01-0{}T00:00:00Z", index + 1),
                ..Default::default()
            };
            store.put("a", &email)?;
            if index < 5 {
                store.defer_body("a", &email, true)?;
            }
        }
        assert_eq!(store.pending("a")?.unwrap().id, "5");
        assert_eq!(store.pending("a")?.unwrap().id, "5");
        Ok(())
    }

    #[test]
    fn status_refresh_updates_archived_mail_and_survives_restart() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("mail.db");
        {
            let db = Store::open(&path)?;
            db.put(
                "a",
                &Email {
                    id: "sent".into(),
                    folder: Folder::Sent,
                    body_loaded: true,
                    subject: "Archived subject".into(),
                    text: Some("Keep the body".into()),
                    html: Some("<p>Keep the body</p>".into()),
                    headers: [("references".into(), "<original>".into())].into(),
                    ..Default::default()
                },
            )?;
            db.mark_read("a", Folder::Sent, "sent")?;
            for status in [
                DeliveryStatus::Sent,
                DeliveryStatus::Delivered,
                DeliveryStatus::Bounced,
            ] {
                db.put(
                    "a",
                    &Email {
                        id: "sent".into(),
                        folder: Folder::Sent,
                        last_event: Some(status),
                        status_checked_at: Some("2026-09-19T10:00:00Z".into()),
                        ..Default::default()
                    },
                )?;
                let saved = db.get("a", Folder::Sent, "sent")?.unwrap();
                assert_eq!(saved.last_event, Some(status));
                assert_eq!(saved.subject, "Archived subject");
                assert_eq!(saved.text.as_deref(), Some("Keep the body"));
                assert_eq!(saved.html.as_deref(), Some("<p>Keep the body</p>"));
                assert_eq!(saved.headers["references"], "<original>");
                assert!(saved.read && saved.body_loaded);
            }
            // Missing status in either list metadata or full content must not
            // erase a previously observed status or advance its check time.
            for body_loaded in [false, true] {
                db.put(
                    "a",
                    &Email {
                        id: "sent".into(),
                        folder: Folder::Sent,
                        body_loaded,
                        text: Some("Keep the body".into()),
                        ..Default::default()
                    },
                )?;
            }
            assert!(db.list("b")?.is_empty());
        }
        let saved = Store::open(&path)?.get("a", Folder::Sent, "sent")?.unwrap();
        assert_eq!(saved.last_event, Some(DeliveryStatus::Bounced));
        assert_eq!(
            saved.status_checked_at.as_deref(),
            Some("2026-09-19T10:00:00Z")
        );
        assert_eq!(saved.text.as_deref(), Some("Keep the body"));
        assert!(saved.read && saved.body_loaded);
        Ok(())
    }

    #[test]
    fn refresh_keeps_body_and_read_state_and_accounts_are_isolated() -> Result<()> {
        let db = Store::open(Path::new(":memory:"))?;
        let email = Email {
            id: "1".into(),
            subject: "Saved".into(),
            text: Some("Keep forever".into()),
            body_loaded: true,
            ..Default::default()
        };
        db.put("a", &email)?;
        db.mark_read("a", Folder::Inbox, "1")?;
        db.put(
            "a",
            &Email {
                id: "1".into(),
                ..Default::default()
            },
        )?;
        let stored = db.get("a", Folder::Inbox, "1")?.unwrap();
        assert_eq!(stored.text.as_deref(), Some("Keep forever"));
        assert!(stored.read && stored.body_loaded);
        assert!(db.list("b")?.is_empty());
        Ok(())
    }
    #[test]
    fn draft_and_sync_checkpoint_survive_reopening() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("test.db");
        let draft = Draft {
            subject: "Unsent".into(),
            idempotency_key: "stable-key".into(),
            ..Default::default()
        };
        {
            let db = Store::open(&path)?;
            db.save_draft("a", &draft)?;
            db.set_setting("a", "cursor", "message-1")?;
        }
        let db = Store::open(&path)?;
        assert_eq!(db.draft("a")?, draft);
        assert_eq!(db.setting("a", "cursor")?.as_deref(), Some("message-1"));
        Ok(())
    }
}
