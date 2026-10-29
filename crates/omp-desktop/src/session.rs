//! Per-session state the view renders, folded from RPC notifications. Kept
//! free of GPUI so it can be tested directly.

use omp_rpc::{
	ExtensionUiRequest, PromptStatus, RpcAgentEvent, RpcNotification, SessionState,
};
use omp_store::Transcript;

/// Whether the agent is working on something the user sent.
#[derive(Debug, Clone, PartialEq)]
pub enum Activity {
	Idle,
	/// A prompt was accepted and has not yielded yet.
	Running,
	/// The prompt yielded but background work may still wake the agent.
	Settling,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
	Info(String),
	Error(String),
}

#[derive(Default)]
pub struct SessionModel {
	pub transcript: Transcript,
	pub tools:      omp_store::tools::ToolRuns,
	/// Row of the assistant message that issued each tool call, so a tool
	/// event can remeasure that row and its result renders inside the card.
	pub tool_rows:  std::collections::HashMap<String, usize>,
	pub activity:   ActivityState,
	pub state:      Option<SessionState>,
	/// Dialogs waiting for an answer, oldest first.
	pub dialogs:    Vec<ExtensionUiRequest>,
	pub notices:    Vec<Notice>,
	pub status:     Option<String>,
	pub queue:      Queue,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Queue {
	pub steering:  Vec<String>,
	pub follow_up: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActivityState(pub Activity);

impl Default for ActivityState {
	fn default() -> Self {
		Self(Activity::Idle)
	}
}

/// What the view must do after a notification.
#[derive(Debug, Default, PartialEq)]
pub struct Effects {
	/// Transcript rows changed: (first changed index, new row count).
	pub rows:          Option<(usize, usize)>,
	/// Session state should be refetched (turn ended, model changed…).
	pub refresh_state: bool,
}

impl SessionModel {
	pub fn activity(&self) -> &Activity {
		&self.activity.0
	}

	pub fn busy(&self) -> bool {
		self.activity.0 != Activity::Idle
	}

	/// Marks a prompt as sent, before the server acknowledges it.
	pub fn prompt_sent(&mut self) {
		self.activity.0 = Activity::Running;
	}

	pub fn apply(&mut self, notification: RpcNotification) -> Effects {
		let mut effects = Effects::default();
		match notification {
			RpcNotification::RpcAgentEvent(event) => self.apply_event(event, &mut effects),
			RpcNotification::PromptResult(result) => {
				self.activity.0 = if result.session_settled { Activity::Idle } else { Activity::Settling };
				if result.status == PromptStatus::Error
					&& let Some(error) = result.error
				{
					self.notices.push(Notice::Error(error.message));
				}
				effects.refresh_state = true;
			},
			RpcNotification::SessionSettled(_) => {
				self.activity.0 = Activity::Idle;
				effects.refresh_state = true;
			},
			RpcNotification::ExtensionUiRequest(request) => self.apply_ui(request),
			RpcNotification::ExtensionError(error) => {
				self.notices.push(Notice::Error(format!("extension {}: {}", error.extension_path, error.error)));
			},
			RpcNotification::RpcFrameError(_) => {
				self.notices.push(Notice::Error("an event exceeded the RPC transport limit and was dropped".into()));
			},
			_ => {},
		}
		effects
	}

	fn apply_event(&mut self, event: RpcAgentEvent, effects: &mut Effects) {
		match &event {
			RpcAgentEvent::AgentStart(_) => self.activity.0 = Activity::Running,
			RpcAgentEvent::ModelChanged(_) | RpcAgentEvent::TurnEnd(_) => effects.refresh_state = true,
			RpcAgentEvent::QueueUpdate(update) => {
				self.queue = Queue { steering: update.steering.clone(), follow_up: update.follow_up.clone() };
			},
			_ => {},
		}
		let rows = self.transcript.entries().len();
		if self.tools.apply(&event)
			&& let Some(id) = tool_call_id(&event)
			&& let Some(&row) = self.tool_rows.get(id)
		{
			effects.rows = Some((row, rows));
		}
		if let Some(change) = self.transcript.apply(&event) {
			let index = match change {
				omp_store::Change::Started { index }
				| omp_store::Change::Updated { index }
				| omp_store::Change::Ended { index, .. } => index,
			};
			for id in tool_call_ids(&self.transcript.entries()[index].message) {
				self.tool_rows.insert(id, index);
			}
			let first = effects.rows.map_or(index, |(row, _)| row.min(index));
			effects.rows = Some((first.min(rows), self.transcript.entries().len()));
		}
	}

	fn apply_ui(&mut self, request: ExtensionUiRequest) {
		match request {
			ExtensionUiRequest::Notify(notify) => self.notices.push(Notice::Info(notify.message)),
			ExtensionUiRequest::SetStatus(status) => self.status = status.status_text.filter(|text| !text.is_empty()),
			ExtensionUiRequest::Cancel(cancel) => self.dialogs.retain(|dialog| dialog_id(dialog) != Some(&cancel.target_id)),
			request @ (ExtensionUiRequest::Select(_)
			| ExtensionUiRequest::Confirm(_)
			| ExtensionUiRequest::Input(_)
			| ExtensionUiRequest::Editor(_)
			| ExtensionUiRequest::Ask(_)) => self.dialogs.push(request),
			_ => {},
		}
	}

	/// Removes an answered dialog.
	pub fn dialog_answered(&mut self, id: &str) {
		self.dialogs.retain(|dialog| dialog_id(dialog) != Some(id));
	}

	/// Number of rows the conversation shows (hidden messages included; the
	/// row renderer collapses them).
	pub fn row_count(&self) -> usize {
		self.transcript.entries().len()
	}
}

pub fn dialog_id(request: &ExtensionUiRequest) -> Option<&str> {
	Some(match request {
		ExtensionUiRequest::Select(r) => &r.id,
		ExtensionUiRequest::Confirm(r) => &r.id,
		ExtensionUiRequest::Input(r) => &r.id,
		ExtensionUiRequest::Editor(r) => &r.id,
		ExtensionUiRequest::Ask(r) => &r.id,
		_ => return None,
	})
}

fn tool_call_id(event: &RpcAgentEvent) -> Option<&str> {
	Some(match event {
		RpcAgentEvent::ToolExecutionStart(e) => &e.tool_call_id,
		RpcAgentEvent::ToolExecutionUpdate(e) => &e.tool_call_id,
		RpcAgentEvent::ToolExecutionEnd(e) => &e.tool_call_id,
		_ => return None,
	})
}

fn tool_call_ids(message: &omp_rpc::AgentMessage) -> Vec<String> {
	let omp_rpc::AgentMessage::Assistant(message) = message else {
		return Vec::new();
	};
	message
		.content
		.iter()
		.flatten()
		.filter_map(|block| match block {
			omp_rpc::AssistantContent::ToolCall(call) => call.id.clone(),
			_ => None,
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use serde_json::{Value, json};

	use super::*;

	fn notify(model: &mut SessionModel, value: Value) -> Effects {
		model.apply(RpcNotification::from_value(value).expect("decodes"))
	}

	#[test]
	fn prompt_lifecycle_tracks_activity() {
		let mut m = SessionModel::default();
		m.prompt_sent();
		assert!(m.busy());
		notify(&mut m, json!({"type":"agent_start"}));
		notify(&mut m, json!({"type":"prompt_result","id":"req_1","agentInvoked":true,"status":"completed","sessionSettled":false}));
		assert_eq!(m.activity(), &Activity::Settling);
		let effects = notify(&mut m, json!({"type":"session_settled"}));
		assert_eq!(m.activity(), &Activity::Idle);
		assert!(effects.refresh_state);
	}

	#[test]
	fn prompt_error_becomes_notice() {
		let mut m = SessionModel::default();
		m.prompt_sent();
		notify(&mut m, json!({"type":"prompt_result","id":"req_1","agentInvoked":true,"status":"error","sessionSettled":true,
			"error":{"message":"overloaded","retryable":true}}));
		assert_eq!(m.notices, [Notice::Error("overloaded".into())]);
		assert!(!m.busy());
	}

	#[test]
	fn dialogs_queue_and_cancel() {
		let mut m = SessionModel::default();
		notify(&mut m, json!({"type":"extension_ui_request","id":"a","method":"confirm","title":"Run?","message":"rm"}));
		notify(&mut m, json!({"type":"extension_ui_request","id":"b","method":"select","title":"Pick","options":["x","y"]}));
		assert_eq!(m.dialogs.len(), 2);
		notify(&mut m, json!({"type":"extension_ui_request","id":"c","method":"cancel","targetId":"a"}));
		assert_eq!(m.dialogs.iter().filter_map(dialog_id).collect::<Vec<_>>(), ["b"]);
		m.dialog_answered("b");
		assert!(m.dialogs.is_empty());
	}

	#[test]
	fn message_rows_report_changes() {
		let mut m = SessionModel::default();
		let effects = notify(&mut m, json!({"type":"message_start","messageId":"m1","message":{"role":"assistant","content":[]}}));
		assert_eq!(effects.rows, Some((0, 1)));
		assert!(m.transcript.entries()[0].streaming);
		let effects = notify(&mut m, json!({"type":"message_end","messageId":"m1","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}));
		// The ended message updates its row in place.
		assert_eq!(effects.rows, Some((0, 1)));
		assert!(!m.transcript.entries()[0].streaming);
	}

	#[test]
	fn tool_output_refreshes_the_row_that_issued_the_call() {
		let mut m = SessionModel::default();
		notify(&mut m, json!({"type":"message_end","messageId":"u","message":{"role":"user","content":"go"}}));
		notify(&mut m, json!({"type":"message_end","messageId":"a","message":{"role":"assistant",
			"content":[{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"ls"}}]}}));
		notify(&mut m, json!({"type":"message_end","messageId":"u2","message":{"role":"user","content":"next"}}));
		notify(&mut m, json!({"type":"tool_execution_start","toolCallId":"c1","toolName":"bash","args":{}}));
		let effects = notify(&mut m, json!({"type":"tool_execution_update","toolCallId":"c1","toolName":"bash",
			"args":{},"partialResult":{"content":[{"type":"text","text":"a.rs"}]}}));
		assert_eq!(effects.rows, Some((1, 3)));
		assert_eq!(m.tools.get("c1").unwrap().output, "a.rs");
	}
}
