//! M0 acceptance: send one prompt, print the reply as it streams, and check
//! that the message rebuilt from deltas matches `message_end`.
//!
//! cargo run -p omp-bridge --example stream -- "your prompt"

use std::io::Write as _;

use omp_bridge::{Session, SessionOptions};
use omp_rpc::{
	AgentMessage, AssistantContent, AssistantMessageEvent, Event, PromptCommand, RpcAgentEvent,
	RpcNotification,
};
use omp_store::{Change, Transcript};
use serde_json::Value;

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let prompt = std::env::args().nth(1).unwrap_or_else(|| {
		"Read Cargo.toml in the current directory, then list the workspace members.".into()
	});
	let session = Session::spawn(&SessionOptions {
		// Only the read-only `read` tool is enabled, so skipping approval prompts
		// cannot write anything; it keeps this unattended run from blocking on a
		// dialog it does not answer.
		extra_args: ["--no-session", "--tools", "read", "--thinking", "low", "--approval-mode", "yolo"]
			.map(String::from)
			.to_vec(),
		..SessionOptions::default()
	})?;
	eprintln!("omp {} · delta={}", session.version, session.delta);

	session.client.call(&PromptCommand { message: prompt, images: None, streaming_behavior: None })?;
	// A stuck run (a dialog nobody answers, a hung provider) fails instead of
	// blocking forever.
	const DEADLINE: std::time::Duration = std::time::Duration::from_secs(180);
	std::thread::spawn(|| {
		std::thread::sleep(DEADLINE);
		eprintln!("\n[timeout] no prompt_result + settle within {}s", DEADLINE.as_secs());
		std::process::exit(2);
	});

	let mut transcript = Transcript::new();
	let (mut ended, mut mismatches, mut prompt_done) = (0, 0, false);
	while let Ok(event) = session.events.recv_blocking() {
		let notification = match event {
			Event::Notification(notification) => notification,
			Event::Undecodable { raw, error } => {
				eprintln!("\n[undecodable {}] {error}", raw["type"]);
				mismatches += 1;
				continue;
			},
			Event::UnmatchedResponse(_) => continue,
		};
		match notification {
			RpcNotification::RpcAgentEvent(event) => {
				print_live(&event);
				if let Some(Change::Ended { index, streamed: Some(streamed) }) = transcript.apply(&event) {
					let final_message = &transcript.entries()[index].message;
					if !matches!(final_message, AgentMessage::Assistant(_)) {
						continue;
					}
					ended += 1;
					let (rebuilt, authoritative) = (summary(&streamed), summary(final_message));
					if rebuilt == authoritative {
						eprintln!("\n[message {} rebuilt == message_end: {} blocks]", index, rebuilt.len());
					} else {
						mismatches += 1;
						eprintln!("\n[MISMATCH message {index}]\n rebuilt: {rebuilt:?}\n final:   {authoritative:?}");
					}
				}
			},
			RpcNotification::ExtensionUiRequest(request) => {
				let value = serde_json::to_value(&request)?;
				eprintln!("\n[ui {}]", value["method"]);
			},
			RpcNotification::PromptResult(result) => {
				eprintln!("\n[prompt_result {:?} settled={}]", result.status, result.session_settled);
				prompt_done = true;
				if result.session_settled {
					break;
				}
			},
			RpcNotification::SessionSettled(_) if prompt_done => break,
			_ => {},
		}
	}

	eprintln!("assistant messages checked: {ended}, mismatches: {mismatches}");
	if let Ok(client) = std::sync::Arc::try_unwrap(session.client) {
		client.close()?;
	}
	if mismatches > 0 || ended == 0 {
		std::process::exit(1);
	}
	Ok(())
}

fn print_live(event: &RpcAgentEvent) {
	let RpcAgentEvent::MessageUpdate(update) = event else {
		return;
	};
	let mut out = std::io::stdout().lock();
	match &update.assistant_message_event {
		AssistantMessageEvent::ThinkingStart(_) => _ = write!(out, "\n[thinking] "),
		AssistantMessageEvent::TextStart(_) => _ = write!(out, "\n[text] "),
		AssistantMessageEvent::ThinkingDelta(e) => _ = write!(out, "{}", e.delta.as_deref().unwrap_or("")),
		AssistantMessageEvent::TextDelta(e) => _ = write!(out, "{}", e.delta.as_deref().unwrap_or("")),
		AssistantMessageEvent::ToolcallEnd(e) => {
			if let Some(call) = &e.tool_call {
				_ = write!(out, "\n[tool {}] {}", call.name.as_deref().unwrap_or("?"),
					Value::Object(call.arguments.clone().unwrap_or_default()));
			}
		},
		_ => {},
	}
	_ = out.flush();
}

/// The content a reader sees: block kinds with their text / call arguments.
fn summary(message: &AgentMessage) -> Vec<String> {
	let AgentMessage::Assistant(message) = message else {
		return Vec::new();
	};
	message
		.content
		.iter()
		.flatten()
		.map(|block| match block {
			AssistantContent::Text(t) => format!("text:{}", t.text.as_deref().unwrap_or("")),
			AssistantContent::Thinking(t) => format!("thinking:{}", t.thinking.as_deref().unwrap_or("")),
			AssistantContent::ToolCall(c) => format!(
				"tool:{}:{}:{}",
				c.id.as_deref().unwrap_or(""),
				c.name.as_deref().unwrap_or(""),
				Value::Object(c.arguments.clone().unwrap_or_default())
			),
			other => format!("{:?}", std::mem::discriminant(other)),
		})
		.collect()
}
