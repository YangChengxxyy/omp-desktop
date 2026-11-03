//! Display projection: turns wire messages into the blocks a view renders,
//! so UI code never matches on wire unions.

use omp_rpc::{AgentMessage, AssistantContent, MessageContent, UserContent};
use serde_json::{Map, Value};

use crate::Entry;

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
	User,
	Assistant,
	/// A tool's result, attached under the call that produced it.
	ToolResult { tool_call_id: String, tool_name: String, is_error: bool },
	/// Messages the conversation view does not render (developer notes,
	/// hooks, summaries…).
	Hidden,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
	/// Markdown source.
	Text(String),
	Thinking(String),
	/// `arguments` is the parsed object once `toolcall_end` arrived, else the
	/// raw JSON fragment streamed so far.
	ToolCall { id: String, name: String, arguments: ToolArgs, intent: Option<String> },
	/// `data` is the base64 payload; empty when the message carried only a
	/// URL or provider file reference.
	Image { mime_type: String, data: String },
	/// Plain text output (tool results).
	Output(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolArgs {
	Complete(Map<String, Value>),
	Partial(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
	pub role:      Role,
	pub blocks:    Vec<Block>,
	pub streaming: bool,
	/// Provider error text for a failed assistant turn.
	pub error:     Option<String>,
}

impl Entry {
	pub fn view(&self) -> View {
		let mut view = View { role: Role::Hidden, blocks: Vec::new(), streaming: self.streaming, error: None };
		match &self.message {
			AgentMessage::User(user) => {
				// Synthetic user messages are omp-injected context, not typed input.
				if user.synthetic == Some(true) {
					return view;
				}
				view.role = Role::User;
				match &user.content {
					Some(MessageContent::String(text)) => view.blocks.push(Block::Text(text.clone())),
					Some(MessageContent::UserContentList(parts)) => view.blocks.extend(parts.iter().map(user_block)),
					None => {},
				}
			},
			AgentMessage::Assistant(message) => {
				view.role = Role::Assistant;
				view.error = message.error_message.clone();
				for (index, block) in message.content.iter().flatten().enumerate() {
					let block = match block {
						AssistantContent::Text(t) => Block::Text(t.text.clone().unwrap_or_default()),
						AssistantContent::Thinking(t) => Block::Thinking(t.thinking.clone().unwrap_or_default()),
						AssistantContent::ToolCall(call) => Block::ToolCall {
							id:        call.id.clone().unwrap_or_default(),
							name:      call.name.clone().unwrap_or_default(),
							arguments: match self.tool_args.get(&index) {
								Some(partial) => ToolArgs::Partial(partial.clone()),
								None => ToolArgs::Complete(call.arguments.clone().unwrap_or_default()),
							},
							intent:    call.intent.clone(),
						},
						AssistantContent::Image(image) => image_block(image),
						_ => continue,
					};
					view.blocks.push(block);
				}
			},
			AgentMessage::ToolResult(result) => {
				view.role = Role::ToolResult {
					tool_call_id: result.tool_call_id.clone().unwrap_or_default(),
					tool_name:    result.tool_name.clone().unwrap_or_default(),
					is_error:     result.is_error == Some(true),
				};
				view.blocks.extend(result.content.iter().flatten().map(|part| match user_block(part) {
					Block::Text(text) => Block::Output(text),
					other => other,
				}));
			},
			_ => {},
		}
		view
	}
}

fn user_block(part: &UserContent) -> Block {
	match part {
		UserContent::Text(t) => Block::Text(t.text.clone().unwrap_or_default()),
		UserContent::Image(image) => image_block(image),
	}
}

fn image_block(image: &omp_rpc::ImageContent) -> Block {
	Block::Image { mime_type: image.mime_type.clone().unwrap_or_default(), data: image.data.clone().unwrap_or_default() }
}

#[cfg(test)]
mod tests {
	use omp_rpc::RpcAgentEvent;
	use serde_json::json;

	use crate::Transcript;

	use super::*;

	fn apply(t: &mut Transcript, value: Value) {
		t.apply(&RpcAgentEvent::from_value(value).expect("decodes"));
	}

	#[test]
	fn streaming_tool_call_shows_partial_then_complete_arguments() {
		let mut t = Transcript::new();
		apply(&mut t, json!({"type":"message_start","messageId":"m","message":{"role":"assistant","content":[]}}));
		apply(&mut t, json!({"type":"message_update","messageId":"m","message":{"role":"assistant"},
			"assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"block":{"type":"toolCall","id":"c","name":"read","arguments":{}}}}));
		apply(&mut t, json!({"type":"message_update","messageId":"m","message":{"role":"assistant"},
			"assistantMessageEvent":{"type":"toolcall_delta","contentIndex":0,"delta":"{\"pa"}}));
		let view = t.entries()[0].view();
		assert!(view.streaming);
		assert!(matches!(&view.blocks[..], [Block::ToolCall { arguments: ToolArgs::Partial(p), .. }] if p == "{\"pa"));

		apply(&mut t, json!({"type":"message_update","messageId":"m","message":{"role":"assistant"},
			"assistantMessageEvent":{"type":"toolcall_end","contentIndex":0,"toolCall":{"type":"toolCall","id":"c","name":"read","arguments":{"path":"x"}}}}));
		let view = t.entries()[0].view();
		assert!(matches!(&view.blocks[..], [Block::ToolCall { arguments: ToolArgs::Complete(a), .. }] if a["path"] == "x"));
	}

	#[test]
	fn roles_and_hidden_messages() {
		let mut t = Transcript::new();
		for (id, message) in [
			("u", json!({"role":"user","content":"hi"})),
			("s", json!({"role":"user","content":"injected","synthetic":true})),
			("r", json!({"role":"toolResult","toolCallId":"c","toolName":"read","content":[{"type":"text","text":"file"}],"isError":true})),
			("d", json!({"role":"developer","content":"note"})),
		] {
			apply(&mut t, json!({"type":"message_end","messageId":id,"message":message}));
		}
		let views: Vec<_> = t.entries().iter().map(Entry::view).collect();
		assert_eq!(views[0].role, Role::User);
		assert_eq!(views[0].blocks, [Block::Text("hi".into())]);
		assert_eq!(views[1].role, Role::Hidden);
		assert_eq!(views[2].role, Role::ToolResult { tool_call_id: "c".into(), tool_name: "read".into(), is_error: true });
		assert_eq!(views[2].blocks, [Block::Output("file".into())]);
		assert_eq!(views[3].role, Role::Hidden);
	}
}
