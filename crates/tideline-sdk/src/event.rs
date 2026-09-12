//! Building an event to record.

use serde::Serialize;
use serde_json::value::RawValue;
use tideline_proto::EventKind;

/// An event on its way to the record. `seq`, `ts`, and both hashes belong to the
/// server; nothing here can set them.
#[derive(Debug, Serialize)]
pub struct NewEvent {
    pub kind: EventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Box<RawValue>>,
}

impl NewEvent {
    pub fn new(kind: EventKind) -> Self {
        NewEvent {
            kind,
            role: None,
            name: None,
            content: None,
            metadata: None,
        }
    }

    pub fn message() -> Self {
        Self::new(EventKind::Message)
    }

    pub fn model_call(name: &str) -> Self {
        Self::new(EventKind::ModelCall).name(name)
    }

    pub fn tool_call(name: &str) -> Self {
        Self::new(EventKind::ToolCall).name(name)
    }

    pub fn decision(name: &str) -> Self {
        Self::new(EventKind::Decision).name(name)
    }

    pub fn role(mut self, role: &str) -> Self {
        self.role = Some(role.to_string());
        self
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    /// Attach structured metadata. Serialised once, here, and sent as those
    /// exact bytes — the server hashes what it receives.
    pub fn metadata<T: Serialize>(mut self, value: &T) -> Self {
        self.metadata = serde_json::to_string(value)
            .ok()
            .and_then(|s| RawValue::from_string(s).ok());
        self
    }
}
