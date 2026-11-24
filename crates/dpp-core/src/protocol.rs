use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_ID: &str = "dpp.events/1";
pub const MAX_EVENT_BYTES: usize = 8 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 4 * 1024;
pub const MAX_PAYLOAD_FIELDS: usize = 16;
pub const MAX_PAYLOAD_NODES: usize = 64;
pub const MAX_STRING_BYTES: usize = 512;
pub const MAX_ARRAY_ITEMS: usize = 16;
pub const MAX_ADAPTER_EVENTS: usize = 64;
pub const MAX_ADAPTER_CAPABILITIES: usize = 16;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ProtocolError(String);

impl ProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterManifest {
    pub adapter: AdapterSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterSection {
    pub id: String,
    pub version: String,
    pub protocol: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterEvent {
    pub protocol: String,
    pub id: u64,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct AdapterSession {
    last_id: Option<u64>,
}

impl AdapterManifest {
    pub fn load(path: &Path) -> Result<Self, ProtocolError> {
        let text = fs::read_to_string(path).map_err(|error| {
            ProtocolError::new(format!("failed to read {}: {error}", path.display()))
        })?;
        let manifest: Self = toml::from_str(&text).map_err(|error| {
            ProtocolError::new(format!("failed to parse {}: {error}", path.display()))
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_adapter_id(&self.adapter.id)?;
        if self.adapter.version.is_empty() || self.adapter.version.len() > 64 {
            return Err(ProtocolError::new("adapter.version must be 1..=64 bytes"));
        }
        if self.adapter.protocol != PROTOCOL_ID {
            return Err(ProtocolError::new(format!(
                "unsupported adapter protocol {:?}; expected {PROTOCOL_ID}",
                self.adapter.protocol
            )));
        }
        if !self
            .adapter
            .capabilities
            .iter()
            .any(|capability| capability == PROTOCOL_ID)
        {
            return Err(ProtocolError::new(format!(
                "adapter must declare capability {PROTOCOL_ID}"
            )));
        }
        if self.adapter.capabilities.len() > MAX_ADAPTER_CAPABILITIES {
            return Err(ProtocolError::new("adapter has too many capabilities"));
        }
        if self.adapter.events.len() > MAX_ADAPTER_EVENTS {
            return Err(ProtocolError::new("adapter has too many events"));
        }
        let mut events = BTreeSet::new();
        for event in &self.adapter.events {
            validate_event_name(event)?;
            if !events.insert(event) {
                return Err(ProtocolError::new(format!(
                    "duplicate adapter event {event:?}"
                )));
            }
        }
        Ok(())
    }

    pub fn allows(&self, event: &str) -> bool {
        self.adapter.events.iter().any(|allowed| allowed == event)
    }
}

impl AdapterEvent {
    pub fn from_json_str(text: &str) -> Result<Self, ProtocolError> {
        if text.len() > MAX_EVENT_BYTES {
            return Err(ProtocolError::new("event payload exceeds wire size limit"));
        }
        let event: Self = serde_json::from_str(text)
            .map_err(|error| ProtocolError::new(format!("invalid event JSON: {error}")))?;
        event.validate()?;
        Ok(event)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol != PROTOCOL_ID {
            return Err(ProtocolError::new(format!(
                "unsupported event protocol {:?}; expected {PROTOCOL_ID}",
                self.protocol
            )));
        }
        if self.id == 0 {
            return Err(ProtocolError::new("event.id must be non-zero"));
        }
        validate_event_name(&self.event)?;
        if let Some(payload) = &self.payload {
            let encoded = serde_json::to_vec(payload)
                .map_err(|error| ProtocolError::new(format!("invalid payload: {error}")))?;
            if encoded.len() > MAX_PAYLOAD_BYTES {
                return Err(ProtocolError::new("payload exceeds size limit"));
            }
            let Value::Object(object) = payload else {
                return Err(ProtocolError::new("payload must be a JSON object"));
            };
            if object.len() > MAX_PAYLOAD_FIELDS {
                return Err(ProtocolError::new("payload has too many fields"));
            }
            let mut nodes = 0;
            validate_value(payload, 0, &mut nodes)?;
        }
        Ok(())
    }
}

impl AdapterSession {
    pub fn accept(
        &mut self,
        manifest: &AdapterManifest,
        event: &AdapterEvent,
    ) -> Result<(), ProtocolError> {
        event.validate()?;
        if !manifest.allows(&event.event) {
            return Err(ProtocolError::new(format!(
                "event {:?} is not declared by adapter {}",
                event.event, manifest.adapter.id
            )));
        }
        if let Some(last) = self.last_id {
            if event.id <= last {
                return Err(ProtocolError::new(format!(
                    "event.id {} is not monotonic after {last}",
                    event.id
                )));
            }
        }
        self.last_id = Some(event.id);
        Ok(())
    }
}

fn validate_adapter_id(id: &str) -> Result<(), ProtocolError> {
    if id.is_empty()
        || id.len() > 64
        || !id.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(ProtocolError::new(format!("invalid adapter id {id:?}")));
    }
    Ok(())
}

fn validate_event_name(name: &str) -> Result<(), ProtocolError> {
    let mut parts = name.split(':');
    let namespace = parts.next().unwrap_or_default();
    let event = parts.next();
    if namespace.is_empty()
        || event.is_none_or(str::is_empty)
        || parts.next().is_some()
        || name.len() > 128
        || !namespace.chars().all(valid_name_char)
        || !event.is_some_and(|event| event.chars().all(valid_name_char))
    {
        return Err(ProtocolError::new(format!("invalid event name {name:?}")));
    }
    Ok(())
}

fn valid_name_char(character: char) -> bool {
    character.is_ascii_lowercase()
        || character.is_ascii_digit()
        || matches!(character, '-' | '_' | '.')
}

fn validate_value(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), ProtocolError> {
    *nodes += 1;
    if *nodes > MAX_PAYLOAD_NODES || depth > 2 {
        return Err(ProtocolError::new("payload exceeds node/depth limit"));
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(text) if text.len() <= MAX_STRING_BYTES => Ok(()),
        Value::String(_) => Err(ProtocolError::new("payload string is too large")),
        Value::Array(values) if values.len() <= MAX_ARRAY_ITEMS => {
            for value in values {
                validate_value(value, depth + 1, nodes)?;
            }
            Ok(())
        }
        Value::Array(_) => Err(ProtocolError::new("payload array has too many items")),
        Value::Object(object) if object.len() <= MAX_PAYLOAD_FIELDS => {
            for (key, value) in object {
                if key.is_empty() || key.len() > 64 {
                    return Err(ProtocolError::new("payload key is invalid"));
                }
                validate_value(value, depth + 1, nodes)?;
            }
            Ok(())
        }
        Value::Object(_) => Err(ProtocolError::new("payload object has too many fields")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
[adapter]
id = "dpp-adapter-paper"
version = "0.1.0"
protocol = "dpp.events/1"
capabilities = ["dpp.events/1"]
events = ["dpp:boss_phase"]
"#;

    fn manifest() -> AdapterManifest {
        toml::from_str(MANIFEST).unwrap()
    }

    #[test]
    fn accepts_valid_manifest_and_event() {
        let manifest = manifest();
        manifest.validate().unwrap();
        let event = AdapterEvent::from_json_str(
            r#"{"protocol":"dpp.events/1","id":1,"event":"dpp:boss_phase","payload":{"phase":2}}"#,
        )
        .unwrap();
        let mut session = AdapterSession::default();
        session.accept(&manifest, &event).unwrap();
    }

    #[test]
    fn rejects_unknown_event_and_non_monotonic_id() {
        let manifest = manifest();
        let event = AdapterEvent::from_json_str(
            r#"{"protocol":"dpp.events/1","id":2,"event":"dpp:other"}"#,
        )
        .unwrap();
        let mut session = AdapterSession::default();
        assert!(session.accept(&manifest, &event).is_err());

        let first = AdapterEvent::from_json_str(
            r#"{"protocol":"dpp.events/1","id":2,"event":"dpp:boss_phase"}"#,
        )
        .unwrap();
        session.accept(&manifest, &first).unwrap();
        assert!(session.accept(&manifest, &first).is_err());
    }

    #[test]
    fn rejects_oversized_payload_and_bad_names() {
        let event = AdapterEvent::from_json_str(&format!(
            r#"{{"protocol":"dpp.events/1","id":1,"event":"dpp:boss_phase","payload":{{"s":"{}"}}}}"#,
            "x".repeat(MAX_STRING_BYTES + 1)
        ));
        assert!(event.is_err());
        let manifest = MANIFEST.replace("dpp:boss_phase", "Dpp:boss");
        assert!(
            toml::from_str::<AdapterManifest>(&manifest)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}
