use serde::{Deserialize, Serialize};

/// The kind of a TLR/1 event. The wire string is hashed into canon, so these
/// strings are part of the protocol and never change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Opens a run; its metadata carries the envelope. Always seq 0.
    RunStarted,
    Message,
    ModelCall,
    ToolCall,
    /// A decision point, including approval requests and resolutions.
    Decision,
    /// Records an erasure: which event, which fields, under what authority.
    Redaction,
    /// Seals a run and carries its final event count.
    RunFinished,
}

impl EventKind {
    pub const ALL: &'static [EventKind] = &[
        EventKind::RunStarted,
        EventKind::Message,
        EventKind::ModelCall,
        EventKind::ToolCall,
        EventKind::Decision,
        EventKind::Redaction,
        EventKind::RunFinished,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::RunStarted => "run_started",
            EventKind::Message => "message",
            EventKind::ModelCall => "model_call",
            EventKind::ToolCall => "tool_call",
            EventKind::Decision => "decision",
            EventKind::Redaction => "redaction",
            EventKind::RunFinished => "run_finished",
        }
    }

    /// Parse a wire string. Unknown kinds return `None` rather than defaulting,
    /// so a mislabelled event cannot enter the record unnoticed.
    pub fn parse(s: &str) -> Option<EventKind> {
        EventKind::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_strings_are_stable() {
        // These strings are hashed into canon. Changing one breaks every
        // record ever written.
        assert_eq!(EventKind::RunStarted.as_str(), "run_started");
        assert_eq!(EventKind::Message.as_str(), "message");
        assert_eq!(EventKind::ModelCall.as_str(), "model_call");
        assert_eq!(EventKind::ToolCall.as_str(), "tool_call");
        assert_eq!(EventKind::Decision.as_str(), "decision");
        assert_eq!(EventKind::Redaction.as_str(), "redaction");
        assert_eq!(EventKind::RunFinished.as_str(), "run_finished");
    }

    #[test]
    fn parses_every_wire_string() {
        for k in EventKind::ALL {
            assert_eq!(EventKind::parse(k.as_str()), Some(*k));
        }
    }

    #[test]
    fn rejects_unknown_kinds() {
        // An unknown kind must fail loudly rather than silently becoming
        // `message`, which would let a writer mislabel an event.
        assert_eq!(EventKind::parse("wire_transfer"), None);
    }

    #[test]
    fn serde_uses_the_wire_strings() {
        let json = serde_json::to_string(&EventKind::ToolCall).unwrap();
        assert_eq!(json, "\"tool_call\"");
        assert_eq!(
            serde_json::from_str::<EventKind>("\"run_finished\"").unwrap(),
            EventKind::RunFinished
        );
    }
}
