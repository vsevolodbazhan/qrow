//! When a message of a conversation includes the assistant notes of its
//! connection. Codex keeps the earlier messages of a conversation, so Qrow
//! sends the notes again only when they are new to the conversation.

use crate::model::SentNotes;
use uuid::Uuid;

/// The notes part of the workspace context of one message.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextNotes {
    /// The conversation has no notes, and it does not need any.
    #[default]
    None,
    /// The notes are new to the conversation. An empty text removes the
    /// notes that the conversation had.
    Send(String),
    /// The conversation already has the current notes.
    Unchanged,
}

/// The notes of a message, and the record to keep after Codex accepts it.
///
/// A message sends the notes when it is the first message with a record,
/// when the connection of the conversation changed, or when the notes
/// changed. It sends empty notes only to remove notes that the conversation
/// had.
pub fn for_message(
    sent: Option<&SentNotes>,
    connection: Option<Uuid>,
    notes: &str,
) -> (ContextNotes, SentNotes) {
    let record = SentNotes {
        connection,
        digest: digest(notes),
    };
    let context = match sent {
        Some(sent) if sent.connection == connection && sent.digest == record.digest => {
            if notes.is_empty() {
                ContextNotes::None
            } else {
                ContextNotes::Unchanged
            }
        }
        Some(sent) if notes.is_empty() && sent.digest == digest("") => ContextNotes::None,
        None if notes.is_empty() => ContextNotes::None,
        _ => ContextNotes::Send(notes.to_owned()),
    };
    (context, record)
}

fn digest(notes: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, notes.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_message_sends_notes_and_later_messages_do_not_repeat_them() {
        let connection = Some(Uuid::new_v4());
        let (context, record) = for_message(None, connection, "Dates are UTC.");
        assert_eq!(context, ContextNotes::Send("Dates are UTC.".into()));
        assert_eq!(record.connection, connection);
        assert_eq!(record.digest.len(), 64);
        let (context, again) = for_message(Some(&record), connection, "Dates are UTC.");
        assert_eq!(context, ContextNotes::Unchanged);
        assert_eq!(again, record);
    }

    #[test]
    fn an_edit_or_another_connection_sends_the_notes_again() {
        let first = Some(Uuid::new_v4());
        let (_, record) = for_message(None, first, "Dates are UTC.");
        let (context, record) = for_message(Some(&record), first, "Dates are local.");
        assert_eq!(context, ContextNotes::Send("Dates are local.".into()));
        // Move to Connection: the same text belongs to another connection.
        let second = Some(Uuid::new_v4());
        let (context, record) = for_message(Some(&record), second, "Dates are local.");
        assert_eq!(context, ContextNotes::Send("Dates are local.".into()));
        // The new connection has no notes: the old notes no longer apply.
        let (context, record) = for_message(Some(&record), second, "");
        assert_eq!(context, ContextNotes::Send(String::new()));
        let (context, _) = for_message(Some(&record), second, "");
        assert_eq!(context, ContextNotes::None);
    }

    #[test]
    fn a_conversation_without_notes_sends_nothing() {
        let (context, record) = for_message(None, None, "");
        assert_eq!(context, ContextNotes::None);
        // A move between connections without notes sends nothing either.
        let (context, _) = for_message(Some(&record), Some(Uuid::new_v4()), "");
        assert_eq!(context, ContextNotes::None);
    }
}
