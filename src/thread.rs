//! Grouping messages into conversations.
//!
//! Mail threads on its Message-IDs: every message carries its own, and names
//! the ones it answers in `In-Reply-To` and `References`. Two messages that
//! mention any identifier in common are part of one conversation, however far
//! apart they sit in the list, and whichever folder they arrived in — a reply
//! you sent belongs with the message it answers.
//!
//! Resend returns headers for received mail only. Mail Receive sent carries
//! whatever [`crate::worker`] recorded as the reply left, and sent mail from
//! before that was recorded carries nothing at all. So messages that link to
//! nothing get a second chance on their subject, guarded by the people
//! involved. That guard matters: a monthly `Invoice` to one supplier would
//! otherwise collapse a year of separate conversations into one. Messages that
//! did link by header are left alone, so a real chain is never merged into
//! another on a coincidence of wording.

use crate::model::{Email, parse_date};
use std::collections::{BTreeSet, HashMap};

/// One conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    /// The oldest message's id, so that the open conversation survives a
    /// resync: new mail arrives newer, and leaves this untouched.
    pub key: String,
    /// Indices into the messages this was built from, oldest first.
    pub messages: Vec<usize>,
}

/// Every conversation in `emails`, newest conversation first.
///
/// Threading reads all of the mail it is given, including archived messages
/// and both folders, so that a chain is never broken by what a view happens to
/// be hiding. Deciding which conversations and which of their messages to show
/// is the caller's.
pub fn group(emails: &[Email]) -> Vec<Thread> {
    let mut union = Union::new(emails.len());

    // Every identifier a message mentions ties it to the first message that
    // mentioned the same one.
    let mut owner: HashMap<String, usize> = HashMap::new();
    for (index, email) in emails.iter().enumerate() {
        for id in email.thread_ids() {
            match owner.entry(id) {
                std::collections::hash_map::Entry::Occupied(entry) => union.join(index, *entry.get()),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(index);
                }
            }
        }
    }

    // Only messages that linked to nothing are eligible for a subject match,
    // so headers always win over a guess.
    let mut linked: HashMap<usize, usize> = HashMap::new();
    for index in 0..emails.len() {
        *linked.entry(union.root(index)).or_default() += 1;
    }
    let mut buckets: HashMap<String, Vec<(BTreeSet<String>, usize)>> = HashMap::new();
    for (index, email) in emails.iter().enumerate() {
        if linked.get(&union.root(index)).copied().unwrap_or_default() > 1 {
            continue;
        }
        let subject = email.normalized_subject();
        if subject.is_empty() {
            continue;
        }
        let people = email.participants();
        if people.is_empty() {
            continue;
        }
        let bucket = buckets.entry(subject).or_default();
        match bucket
            .iter_mut()
            .find(|(known, _)| !known.is_disjoint(&people))
        {
            Some((known, first)) => {
                known.extend(people);
                union.join(index, *first);
            }
            None => bucket.push((people, index)),
        }
    }

    let mut threads: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..emails.len() {
        threads.entry(union.root(index)).or_default().push(index);
    }
    let mut threads: Vec<Thread> = threads
        .into_values()
        .map(|mut messages| {
            messages.sort_by_key(|&index| ordering(&emails[index]));
            Thread {
                key: emails[messages[0]].id.clone(),
                messages,
            }
        })
        .collect();
    // Newest conversation first, matching the order mail is listed in.
    threads.sort_by(|a, b| {
        ordering(&emails[*b.messages.last().expect("a thread has a message")])
            .cmp(&ordering(&emails[*a.messages.last().expect("a thread has a message")]))
    });
    threads
}

/// When a message was sent, falling back to its id so that messages with an
/// unreadable or identical date still order the same way every time.
fn ordering(email: &Email) -> (i64, &str) {
    (
        parse_date(&email.created_at)
            .map(|date| date.timestamp_micros())
            .unwrap_or(i64::MIN),
        &email.id,
    )
}

/// Disjoint-set union over message positions.
struct Union {
    parent: Vec<usize>,
}

impl Union {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }
    fn root(&mut self, mut index: usize) -> usize {
        while self.parent[index] != index {
            self.parent[index] = self.parent[self.parent[index]];
            index = self.parent[index];
        }
        index
    }
    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            self.parent[a] = b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Folder;

    fn email(id: &str, subject: &str, from: &str, to: &str, day: u32) -> Email {
        Email {
            id: id.into(),
            from: from.into(),
            to: vec![to.into()],
            subject: subject.into(),
            created_at: format!("2026-04-{day:02}T10:00:00Z"),
            message_id: Some(format!("<{id}@example.com>")),
            ..Default::default()
        }
    }

    fn replying(mut email: Email, to: &[&str]) -> Email {
        email.headers.insert(
            "References".into(),
            to.iter()
                .map(|id| format!("<{id}@example.com>"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        email
    }

    /// The point of threading: a reply sits with the message it answers even
    /// though the two are in different folders.
    #[test]
    fn a_sent_reply_joins_the_received_message_it_answers() {
        let mut ours = replying(
            email("ours", "Re: Notes", "you@mine.test", "maya@example.com", 2),
            &["theirs"],
        );
        ours.folder = Folder::Sent;
        let emails = vec![
            email("theirs", "Notes", "maya@example.com", "you@mine.test", 1),
            ours,
            email("other", "Something else", "alex@example.com", "you@mine.test", 3),
        ];
        let threads = group(&emails);
        assert_eq!(threads.len(), 2);
        let conversation = threads
            .iter()
            .find(|thread| thread.messages.len() == 2)
            .expect("the reply threads with the original");
        assert_eq!(conversation.messages, [0, 1]);
        assert_eq!(conversation.key, "theirs");
    }

    /// A chain holds together through its `References` even when the middle of
    /// it is missing, and prefixes stacked by other clients do not split it.
    #[test]
    fn a_broken_chain_still_threads_through_shared_references() {
        let emails = vec![
            email("a", "Plan", "maya@example.com", "you@mine.test", 1),
            replying(
                email("c", "Re: Fwd: Plan", "alex@example.com", "you@mine.test", 3),
                &["a", "b"],
            ),
        ];
        let threads = group(&emails);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].messages, [0, 1]);
    }

    /// Sent mail from before Receive recorded reply headers has no linkage at
    /// all, so the subject has to carry it — but only between people who were
    /// actually writing to each other.
    #[test]
    fn headerless_mail_threads_on_subject_only_with_shared_participants() {
        let mut ours = email("ours", "Re: Budget", "you@mine.test", "maya@example.com", 2);
        ours.folder = Folder::Sent;
        ours.message_id = None;
        let emails = vec![
            email("theirs", "Budget", "maya@example.com", "you@mine.test", 1),
            ours,
            // Same subject, nobody in common: a different conversation.
            email("stranger", "Budget", "sam@other.test", "someone@else.test", 3),
        ];
        let threads = group(&emails);
        assert_eq!(threads.len(), 2);
        assert!(threads.iter().any(|thread| thread.messages == [0, 1]));
        assert!(threads.iter().any(|thread| thread.messages == [2]));
    }

    /// The guard that keeps a recurring subject from collapsing into one
    /// conversation: messages already threaded by header are never merged by
    /// wording, however often that wording repeats.
    #[test]
    fn a_repeated_subject_does_not_merge_conversations_that_have_headers() {
        let emails = vec![
            replying(
                email("jan-reply", "Re: Invoice", "you@mine.test", "billing@supplier.test", 2),
                &["jan"],
            ),
            email("jan", "Invoice", "billing@supplier.test", "you@mine.test", 1),
            replying(
                email("feb-reply", "Re: Invoice", "you@mine.test", "billing@supplier.test", 4),
                &["feb"],
            ),
            email("feb", "Invoice", "billing@supplier.test", "you@mine.test", 3),
        ];
        let threads = group(&emails);
        assert_eq!(threads.len(), 2);
        for thread in &threads {
            assert_eq!(thread.messages.len(), 2);
        }
    }

    /// Conversations are listed newest first and read oldest first, and a
    /// conversation keeps its name when the next reply lands.
    #[test]
    fn threads_are_ordered_newest_first_and_keyed_on_their_oldest_message() {
        let first = email("first", "Old", "maya@example.com", "you@mine.test", 1);
        let newer = email("newer", "New", "alex@example.com", "you@mine.test", 5);
        let threads = group(&[first.clone(), newer.clone()]);
        assert_eq!(threads[0].key, "newer");
        assert_eq!(threads[1].key, "first");

        let reply = replying(
            email("reply", "Re: Old", "you@mine.test", "maya@example.com", 9),
            &["first"],
        );
        let threads = group(&[first, newer, reply]);
        let conversation = threads
            .iter()
            .find(|thread| thread.messages.len() == 2)
            .expect("the reply joins the older conversation");
        assert_eq!(conversation.key, "first");
        // The conversation is now the most recently active one.
        assert_eq!(threads[0].key, "first");
    }

    /// Mail that says nothing about its identity threads alone rather than
    /// collapsing into one conversation of everything unidentifiable.
    #[test]
    fn messages_without_identity_or_subject_stand_alone() {
        let emails = vec![
            Email {
                id: "one".into(),
                ..Default::default()
            },
            Email {
                id: "two".into(),
                ..Default::default()
            },
        ];
        let threads = group(&emails);
        assert_eq!(threads.len(), 2);
        assert!(group(&[]).is_empty());
    }
}
