//! UI-independent conversation state rebuilt from omp RPC session events.
//!
//! [`Transcript`] consumes `message_start` / `message_update` / `message_end`
//! in either `set_event_filter` mode:
//! - full snapshots: every `message_update` carries the whole message so far,
//!   which replaces the stored one;
//! - `messageUpdates: "delta"`: the update carries `message: { role }` only,
//!   and the assistant event (block shells, text/thinking deltas, tool-call
//!   ends) is applied to the stored message.
//!
//! `message_end` is authoritative in both modes and replaces whatever the
//! stream produced.

pub mod display;
pub mod sessions;
pub mod tools;

use std::collections::HashMap;

use omp_rpc::{
	AgentMessage, AssistantContent, AssistantMessage, AssistantMessageEvent, MessageEndEvent,
	MessageStartEvent, MessageUpdateEvent, RpcAgentEvent, TextContent, ThinkingContent,
};
use serde_json::{Map, Value};

/// `customType` of the local rows that hold slash-command output.
pub const COMMAND_OUTPUT: &str = "omp-desktop/command-output";

/// One message in display order.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
	/// The RPC `messageId`, or a local id when the server sent none.
	pub id:        String,
	pub message:   AgentMessage,
	/// Between `message_start` and `message_end`.
	pub streaming: bool,
	/// Raw argument JSON streamed so far for tool calls that have not ended,
	/// keyed by content index. Complete arguments arrive in `toolcall_end`.
	pub tool_args: HashMap<usize, String>,
}

/// What an applied event changed, for incremental rendering.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
	Started { index: usize },
	Updated { index: usize },
	/// `streamed` is the message as rebuilt from the stream, just before
	/// `message_end` replaced it (`None` when the message had not started).
	Ended { index: usize, streamed: Option<AgentMessage> },
}

#[derive(Debug, Default)]
pub struct Transcript {
	entries:    Vec<Entry>,
	by_id:      HashMap<String, usize>,
	next_local: u64,
}

impl Transcript {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn entries(&self) -> &[Entry] {
		&self.entries
	}

	/// Applies one session event; events other than message lifecycle return
	/// `None`.
	pub fn apply(&mut self, event: &RpcAgentEvent) -> Option<Change> {
		match event {
			RpcAgentEvent::MessageStart(start) => Some(self.start(start)),
			RpcAgentEvent::MessageUpdate(update) => self.update(update),
			RpcAgentEvent::MessageEnd(end) => Some(self.end(end)),
			_ => None,
		}
	}

	fn start(&mut self, start: &MessageStartEvent) -> Change {
		let id = match &start.message_id {
			Some(id) => id.clone(),
			None => self.local_id(),
		};
		let entry = Entry {
			id:        id.clone(),
			message:   start.message.clone(),
			streaming: true,
			tool_args: HashMap::new(),
		};
		let index = match self.by_id.get(&id) {
			Some(&index) => {
				self.entries[index] = entry;
				index
			},
			None => {
				self.entries.push(entry);
				self.by_id.insert(id, self.entries.len() - 1);
				self.entries.len() - 1
			},
		};
		Change::Started { index }
	}

	fn update(&mut self, update: &MessageUpdateEvent) -> Option<Change> {
		let index = self.resolve(update.message_id.as_deref())?;
		let entry = &mut self.entries[index];
		// Full-snapshot mode: the frame carries the message so far.
		if let AgentMessage::Assistant(snapshot) = &update.message
			&& snapshot.content.is_some()
		{
			entry.message = update.message.clone();
			return Some(Change::Updated { index });
		}
		let AgentMessage::Assistant(message) = &mut entry.message else {
			return None;
		};
		apply_delta(message, &mut entry.tool_args, &update.assistant_message_event);
		Some(Change::Updated { index })
	}

	fn end(&mut self, end: &MessageEndEvent) -> Change {
		match self.resolve(end.message_id.as_deref()) {
			Some(index) => {
				let entry = &mut self.entries[index];
				let streamed = std::mem::replace(&mut entry.message, end.message.clone());
				entry.streaming = false;
				entry.tool_args.clear();
				Change::Ended { index, streamed: Some(streamed) }
			},
			None => {
				let id = end.message_id.clone().unwrap_or_else(|| self.local_id());
				self.entries.push(Entry {
					id:        id.clone(),
					message:   end.message.clone(),
					streaming: false,
					tool_args: HashMap::new(),
				});
				let index = self.entries.len() - 1;
				self.by_id.insert(id, index);
				Change::Ended { index, streamed: None }
			},
		}
	}

	/// The entry for `id`, or the newest streaming entry when the server sent
	/// no id.
	fn resolve(&self, id: Option<&str>) -> Option<usize> {
		match id {
			Some(id) => self.by_id.get(id).copied(),
			None => self.entries.iter().rposition(|entry| entry.streaming),
		}
	}

	/// Appends the text of a `command_output` notification as a local row;
	/// returns its index.
	pub fn push_command_output(&mut self, text: &str) -> usize {
		let id = self.local_id();
		let message = AgentMessage::Custom(omp_rpc::CustomMessage {
			role: Some(omp_rpc::LitCustom),
			custom_type: Some(COMMAND_OUTPUT.to_owned()),
			content: Some(omp_rpc::MessageContent::String(text.to_owned())),
			display: Some(true),
			..Default::default()
		});
		self.entries.push(Entry { id: id.clone(), message, streaming: false, tool_args: HashMap::new() });
		self.by_id.insert(id, self.entries.len() - 1);
		self.entries.len() - 1
	}

	fn local_id(&mut self) -> String {
		self.next_local += 1;
		format!("local-{}", self.next_local)
	}
}

fn apply_delta(
	message: &mut AssistantMessage,
	tool_args: &mut HashMap<usize, String>,
	event: &AssistantMessageEvent,
) {
	use AssistantMessageEvent as E;
	match event {
		// A restart of the streaming message carries the whole message.
		E::Start(start) => {
			if let Some(AgentMessage::Assistant(restart)) =
				start.extra.get("message").cloned().and_then(|v| AgentMessage::from_value(v).ok())
			{
				*message = restart;
				tool_args.clear();
			}
		},
		E::TextStart(e) => set_block(message, e.content_index, &e.extra),
		E::ThinkingStart(e) => set_block(message, e.content_index, &e.extra),
		E::ToolcallStart(e) => set_block(message, e.content_index, &e.extra),
		E::TextDelta(e) => {
			if let (Some(AssistantContent::Text(block)), Some(delta)) =
				(block_mut(message, e.content_index), &e.delta)
			{
				block.text.get_or_insert_default().push_str(delta);
			}
		},
		E::ThinkingDelta(e) => {
			if let (Some(AssistantContent::Thinking(block)), Some(delta)) =
				(block_mut(message, e.content_index), &e.delta)
			{
				block.thinking.get_or_insert_default().push_str(delta);
			}
		},
		E::TextEnd(e) => {
			if let Some(block) = end_block(message, e.content_index, &e.extra) {
				let mut text = match block {
					AssistantContent::Text(text) => text,
					_ => TextContent::default(),
				};
				text.text = e.content.clone();
				put(message, e.content_index, AssistantContent::Text(text));
			}
		},
		E::ThinkingEnd(e) => {
			if let Some(block) = end_block(message, e.content_index, &e.extra) {
				let mut thinking = match block {
					AssistantContent::Thinking(thinking) => thinking,
					_ => ThinkingContent::default(),
				};
				thinking.thinking = e.content.clone();
				put(message, e.content_index, AssistantContent::Thinking(thinking));
			}
		},
		E::ToolcallDelta(e) => {
			if let (Some(index), Some(delta)) = (to_index(e.content_index), &e.delta) {
				tool_args.entry(index).or_default().push_str(delta);
			}
		},
		E::ToolcallEnd(e) => {
			if let Some(call) = &e.tool_call {
				put(message, e.content_index, AssistantContent::ToolCall(call.clone()));
			}
			if let Some(index) = to_index(e.content_index) {
				tool_args.remove(&index);
			}
		},
		E::ImageEnd(e) => {
			if let Some(image) = &e.content {
				put(message, e.content_index, AssistantContent::Image(image.clone()));
			}
		},
		// `usage`, `stopReason`, and errors arrive in `message_end`.
		E::Done(_) | E::Error(_) => {},
	}
}

/// Installs the block shell a `*_start` carries; its streamed content is
/// empty so the following deltas apply from scratch.
fn set_block(message: &mut AssistantMessage, index: Option<i64>, extra: &Map<String, Value>) {
	if let Some(block) = parse_block(extra) {
		put(message, index, block);
	}
}

/// The block a `*_end` describes: its `block` metadata when sent, else the
/// stored block. `None` when neither exists.
fn end_block(
	message: &mut AssistantMessage,
	index: Option<i64>,
	extra: &Map<String, Value>,
) -> Option<AssistantContent> {
	parse_block(extra).or_else(|| block_mut(message, index).map(|block| block.clone()))
		.or(Some(AssistantContent::Text(TextContent::default())))
}

fn parse_block(extra: &Map<String, Value>) -> Option<AssistantContent> {
	AssistantContent::from_value(extra.get("block")?.clone()).ok()
}

fn to_index(index: Option<i64>) -> Option<usize> {
	usize::try_from(index?).ok()
}

fn block_mut(message: &mut AssistantMessage, index: Option<i64>) -> Option<&mut AssistantContent> {
	message.content.as_mut()?.get_mut(to_index(index)?)
}

/// Stores `block` at `index`, padding with empty text blocks if the stream
/// skipped ahead.
fn put(message: &mut AssistantMessage, index: Option<i64>, block: AssistantContent) {
	let Some(index) = to_index(index) else {
		return;
	};
	let content = message.content.get_or_insert_default();
	if index >= content.len() {
		content.resize_with(index + 1, || AssistantContent::Text(TextContent::default()));
	}
	content[index] = block;
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	fn event(value: Value) -> RpcAgentEvent {
		RpcAgentEvent::from_value(value).expect("event decodes")
	}

	fn update(id: &str, assistant_event: Value) -> RpcAgentEvent {
		event(json!({
			"type": "message_update",
			"messageId": id,
			"message": { "role": "assistant" },
			"assistantMessageEvent": assistant_event,
		}))
	}

	fn texts(message: &AgentMessage) -> Vec<String> {
		let AgentMessage::Assistant(message) = message else {
			panic!("not assistant")
		};
		message
			.content
			.iter()
			.flatten()
			.map(|block| match block {
				AssistantContent::Text(t) => format!("text:{}", t.text.as_deref().unwrap_or("")),
				AssistantContent::Thinking(t) => {
					format!("thinking:{}", t.thinking.as_deref().unwrap_or(""))
				},
				AssistantContent::ToolCall(c) => format!(
					"tool:{}:{}",
					c.name.as_deref().unwrap_or(""),
					Value::Object(c.arguments.clone().unwrap_or_default())
				),
				other => format!("{other:?}"),
			})
			.collect()
	}

	fn start(t: &mut Transcript, id: &str) {
		t.apply(&event(json!({
			"type": "message_start",
			"messageId": id,
			"message": { "role": "assistant", "content": [] },
		})));
	}

	#[test]
	fn delta_stream_rebuilds_thinking_text_and_tool_call() {
		let mut t = Transcript::new();
		start(&mut t, "m1");
		for e in [
			json!({"type":"thinking_start","contentIndex":0,"block":{"type":"thinking","thinking":""}}),
			json!({"type":"thinking_delta","contentIndex":0,"delta":"plan "}),
			json!({"type":"thinking_delta","contentIndex":0,"delta":"it"}),
			json!({"type":"text_start","contentIndex":1,"block":{"type":"text","text":""}}),
			json!({"type":"text_delta","contentIndex":1,"delta":"你好"}),
			json!({"type":"text_delta","contentIndex":1,"delta":"，世界"}),
			json!({"type":"toolcall_start","contentIndex":2,"block":{"type":"toolCall","id":"c1","name":"read","arguments":{}}}),
			json!({"type":"toolcall_delta","contentIndex":2,"delta":"{\"path\":"}),
		] {
			t.apply(&update("m1", e));
		}
		assert_eq!(t.entries()[0].tool_args.get(&2).map(String::as_str), Some("{\"path\":"));
		t.apply(&update(
			"m1",
			json!({"type":"toolcall_end","contentIndex":2,"toolCall":{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"a.rs"}}}),
		));

		let entry = &t.entries()[0];
		assert!(entry.streaming);
		assert!(entry.tool_args.is_empty());
		assert_eq!(texts(&entry.message), [
			"thinking:plan it",
			"text:你好，世界",
			r#"tool:read:{"path":"a.rs"}"#
		]);
	}

	#[test]
	fn start_shell_is_emptied_even_if_snapshot_ran_ahead() {
		// The server may snapshot the block after some text was produced; the
		// shell it sends is emptied, so deltas must not double the text.
		let mut t = Transcript::new();
		start(&mut t, "m1");
		t.apply(&update("m1", json!({"type":"text_start","contentIndex":0,"block":{"type":"text","text":""}})));
		t.apply(&update("m1", json!({"type":"text_delta","contentIndex":0,"delta":"ab"})));
		t.apply(&update("m1", json!({"type":"text_end","contentIndex":0,"content":"abc","block":{"type":"text","textSignature":"sig"}})));
		let AgentMessage::Assistant(m) = &t.entries()[0].message else { panic!() };
		let Some(AssistantContent::Text(block)) = m.content.as_ref().and_then(|c| c.first()) else {
			panic!()
		};
		// `text_end` content wins over the accumulated deltas and keeps metadata.
		assert_eq!(block.text.as_deref(), Some("abc"));
		assert_eq!(block.text_signature.as_deref(), Some("sig"));
	}

	#[test]
	fn full_snapshot_updates_replace_message() {
		let mut t = Transcript::new();
		start(&mut t, "m1");
		t.apply(&event(json!({
			"type": "message_update",
			"messageId": "m1",
			"message": { "role": "assistant", "content": [{"type":"text","text":"1\n2"}] },
			"assistantMessageEvent": {"type":"text_delta","contentIndex":0,"delta":"2"},
		})));
		assert_eq!(texts(&t.entries()[0].message), ["text:1\n2"]);
	}

	#[test]
	fn message_end_is_authoritative_and_reports_streamed_version() {
		let mut t = Transcript::new();
		start(&mut t, "m1");
		t.apply(&update("m1", json!({"type":"text_start","contentIndex":0,"block":{"type":"text","text":""}})));
		t.apply(&update("m1", json!({"type":"text_delta","contentIndex":0,"delta":"draft"})));
		let change = t.apply(&event(json!({
			"type": "message_end",
			"messageId": "m1",
			"message": { "role": "assistant", "content": [{"type":"text","text":"final"}], "stopReason": "stop" },
		})));
		let Some(Change::Ended { index: 0, streamed: Some(streamed) }) = change else {
			panic!("{change:?}")
		};
		assert_eq!(texts(&streamed), ["text:draft"]);
		assert_eq!(texts(&t.entries()[0].message), ["text:final"]);
		assert!(!t.entries()[0].streaming);
	}

	#[test]
	fn interleaved_messages_stay_separate_by_id() {
		let mut t = Transcript::new();
		start(&mut t, "m1");
		start(&mut t, "m2");
		for id in ["m1", "m2"] {
			t.apply(&update(id, json!({"type":"text_start","contentIndex":0,"block":{"type":"text","text":""}})));
		}
		t.apply(&update("m2", json!({"type":"text_delta","contentIndex":0,"delta":"two"})));
		t.apply(&update("m1", json!({"type":"text_delta","contentIndex":0,"delta":"one"})));
		assert_eq!(texts(&t.entries()[0].message), ["text:one"]);
		assert_eq!(texts(&t.entries()[1].message), ["text:two"]);
	}

	#[test]
	fn skipped_index_is_padded() {
		let mut t = Transcript::new();
		start(&mut t, "m1");
		t.apply(&update("m1", json!({"type":"text_start","contentIndex":2,"block":{"type":"text","text":""}})));
		t.apply(&update("m1", json!({"type":"text_delta","contentIndex":2,"delta":"x"})));
		assert_eq!(texts(&t.entries()[0].message), ["text:", "text:", "text:x"]);
	}
}
