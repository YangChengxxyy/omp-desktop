//! Per-session state the view renders, folded from RPC notifications. Kept
//! free of GPUI so it can be tested directly.

use omp_rpc::{
	ExtensionUiRequest, PromptStatus, RpcAgentEvent, RpcNotification, SessionState, SubagentLifecycleStatus,
	SubagentStatus,
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
	/// Subagents in launch order; rows stay after the run ends so the panel
	/// can show the final state.
	pub subagents:  Vec<SubagentRow>,
	/// Control request in flight, if any: the controls stay disabled until
	/// the matching notification arrives (`set_model` → `model_changed`, …).
	pub pending:    Option<PendingControl>,
	/// A compaction (manual or automatic) is running.
	pub compacting: bool,
	/// Every queue state seen, for tests: a steer is delivered within moments,
	/// so the current queue alone cannot show that it was queued.
	#[cfg(test)]
	pub queue_log:  Vec<Queue>,
}

/// One subagent run in the panel. Lifecycle and progress frames share
/// `index`; only lifecycle carries the `id` that `cancel_subagent` takes.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentRow {
	pub index:  i64,
	pub id:     Option<String>,
	pub agent:  String,
	pub status: SubagentStatus,
	/// Latest task text (`description` at start, then the progress `task`).
	pub task:   Option<String>,
	/// Raw `AgentProgress` record from the latest `subagent_progress`.
	pub progress: Option<serde_json::Map<String, serde_json::Value>>,
}

impl SubagentRow {
	pub fn running(&self) -> bool {
		matches!(self.status, SubagentStatus::Pending | SubagentStatus::Running)
	}
}

/// A control command in flight; its controls stay disabled until the call
/// returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingControl {
	Model,
	Thinking,
	Compact,
	AutoCompact,
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
			RpcNotification::SubagentLifecycle(event) => self.apply_subagent_lifecycle(event.payload),
			RpcNotification::SubagentProgress(event) => self.apply_subagent_progress(event.payload),
			RpcNotification::SessionInfoUpdate(update) => {
				if let Some(state) = &mut self.state {
					state.session_name = update.title;
				}
			},
			RpcNotification::ConfigUpdate(_) => effects.refresh_state = true,
			_ => {},
		}
		effects
	}

	fn apply_event(&mut self, event: RpcAgentEvent, effects: &mut Effects) {
		match &event {
			RpcAgentEvent::AgentStart(_) => self.activity.0 = Activity::Running,
			RpcAgentEvent::ModelChanged(_) | RpcAgentEvent::TurnEnd(_) | RpcAgentEvent::AgentEnd(_) => {
				effects.refresh_state = true;
			},
			RpcAgentEvent::ThinkingLevelChanged(event) => {
				if let Some(state) = &mut self.state {
					state.thinking_level = event.thinking_level;
				}
			},
			RpcAgentEvent::AutoCompactionStart(_) | RpcAgentEvent::ManualCompactionStart(_) => {
				self.compacting = true;
			},
			RpcAgentEvent::AutoCompactionEnd(_) => {
				self.compacting = false;
				effects.refresh_state = true;
			},
			RpcAgentEvent::ManualCompactionEnd(end) => {
				self.compacting = false;
				effects.refresh_state = true;
				if let Some(error) = &end.error_message
					&& !end.aborted
				{
					self.notices.push(Notice::Info(format!("压缩未执行：{error}")));
				}
			},
			RpcAgentEvent::TodoAutoClear(_) => {
				if let Some(state) = &mut self.state {
					state.todo_phases.clear();
				}
			},
			RpcAgentEvent::QueueUpdate(update) => {
				self.queue = Queue { steering: update.steering.clone(), follow_up: update.follow_up.clone() };
				#[cfg(test)]
				self.queue_log.push(self.queue.clone());
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

	fn subagent_row(&mut self, index: i64, agent: &str) -> &mut SubagentRow {
		let position = match self.subagents.iter().position(|row| row.index == index) {
			Some(position) => position,
			None => {
				self.subagents.push(SubagentRow {
					index,
					id: None,
					agent: agent.to_owned(),
					status: SubagentStatus::Pending,
					task: None,
					progress: None,
				});
				self.subagents.len() - 1
			},
		};
		&mut self.subagents[position]
	}

	fn apply_subagent_lifecycle(&mut self, payload: omp_rpc::SubagentLifecyclePayload) {
		let row = self.subagent_row(payload.index, &payload.agent);
		row.id = Some(payload.id);
		row.status = match payload.status {
			SubagentLifecycleStatus::Started => SubagentStatus::Running,
			SubagentLifecycleStatus::Completed => SubagentStatus::Completed,
			SubagentLifecycleStatus::Failed => SubagentStatus::Failed,
			SubagentLifecycleStatus::Aborted => SubagentStatus::Aborted,
		};
		if row.task.is_none() {
			row.task = payload.description;
		}
	}

	fn apply_subagent_progress(&mut self, payload: omp_rpc::SubagentProgressPayload) {
		let row = self.subagent_row(payload.index, &payload.agent);
		// Progress can trail the `completed` lifecycle frame; keep the end state.
		if row.status == SubagentStatus::Pending {
			row.status = SubagentStatus::Running;
		}
		row.task = Some(payload.task);
		row.progress = Some(payload.progress);
	}

	/// Replaces the subagent rows with a `get_subagents` snapshot (on connect
	/// or reconnect, when lifecycle frames may have been missed).
	pub fn load_subagents(&mut self, snapshots: Vec<omp_rpc::SubagentSnapshot>) {
		self.subagents = snapshots
			.into_iter()
			.map(|s| SubagentRow {
				index:    s.index,
				id:       Some(s.id),
				agent:    s.agent,
				status:   s.status,
				task:     s.task.or(s.description),
				progress: s.progress,
			})
			.collect();
		self.subagents.sort_by_key(|row| row.index);
	}

	pub fn running_subagents(&self) -> usize {
		self.subagents.iter().filter(|row| row.running()).count()
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

	/// Replaces the conversation with a resumed session's history. Each
	/// message goes through `message_end`, so tool results attach to their
	/// cards exactly as they do live.
	pub fn load_history(&mut self, messages: Vec<omp_rpc::AgentMessage>) {
		self.transcript = Transcript::default();
		self.tools = Default::default();
		self.tool_rows.clear();
		for message in messages {
			let end = RpcAgentEvent::MessageEnd(omp_rpc::MessageEndEvent { message, message_id: None });
			self.apply_event(end, &mut Effects::default());
		}
	}

	/// Whether a dialog waits for the user (the tab shows a warning dot).
	pub fn needs_attention(&self) -> bool {
		!self.dialogs.is_empty()
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
	#[test]
	fn history_attaches_tool_results_to_their_call() {
		let mut m = SessionModel::default();
		let messages = [
			json!({"role":"user","content":[{"type":"text","text":"ls"}],"timestamp":1}),
			json!({"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"ls"}}],"timestamp":2}),
			json!({"role":"toolResult","toolCallId":"c1","toolName":"bash","content":[{"type":"text","text":"a.rs"}],"isError":false,"timestamp":3}),
		]
		.map(|v| omp_rpc::AgentMessage::from_value(v).expect("message"));
		m.load_history(messages.to_vec());
		assert_eq!(m.row_count(), 3);
		assert_eq!(m.tool_rows.get("c1"), Some(&1));
		assert_eq!(m.tools.get("c1").unwrap().output, "a.rs");
		assert!(!m.busy());
		// Reloading replaces rather than appends.
		m.load_history(messages[..1].to_vec());
		assert_eq!(m.row_count(), 1);
		assert!(m.tools.get("c1").is_none());
	}

	#[test]
	fn subagent_frames_merge_by_index() {
		let mut m = SessionModel::default();
		notify(&mut m, json!({"type":"subagent_progress","payload":{"index":0,"agent":"explore","agentSource":"bundled",
			"task":"scan repo","progress":{"toolCount":2}}}));
		assert_eq!(m.subagents.len(), 1);
		assert_eq!(m.subagents[0].status, SubagentStatus::Running);
		assert_eq!(m.subagents[0].id, None, "progress frames carry no id");
		notify(&mut m, json!({"type":"subagent_lifecycle","payload":{"id":"sa-1","agent":"explore","agentSource":"bundled",
			"status":"started","index":0,"description":"desc"}}));
		notify(&mut m, json!({"type":"subagent_lifecycle","payload":{"id":"sa-2","agent":"task","agentSource":"bundled",
			"status":"started","index":1}}));
		assert_eq!(m.subagents.len(), 2);
		assert_eq!(m.subagents[0].id.as_deref(), Some("sa-1"));
		assert_eq!(m.subagents[0].task.as_deref(), Some("scan repo"), "progress task wins over description");
		assert_eq!(m.running_subagents(), 2);
		notify(&mut m, json!({"type":"subagent_lifecycle","payload":{"id":"sa-1","agent":"explore","agentSource":"bundled",
			"status":"aborted","index":0}}));
		// A late progress frame must not resurrect a finished run.
		notify(&mut m, json!({"type":"subagent_progress","payload":{"index":0,"agent":"explore","agentSource":"bundled",
			"task":"scan repo","progress":{}}}));
		assert_eq!(m.subagents[0].status, SubagentStatus::Aborted);
		assert_eq!(m.running_subagents(), 1);
	}

	#[test]
	fn compaction_and_state_events() {
		let mut m = SessionModel::default();
		m.state = Some(serde_json::from_value(json!({"sessionId":"s","thinkingLevel":"high",
			"todoPhases":[{"name":"p","tasks":[{"content":"a","status":"pending"}]}]})).unwrap());
		notify(&mut m, json!({"type":"manual_compaction_start"}));
		assert!(m.compacting);
		let effects = notify(&mut m, json!({"type":"manual_compaction_end","aborted":false,"skipped":true,
			"errorMessage":"nothing to compact"}));
		assert!(!m.compacting);
		assert!(effects.refresh_state);
		assert_eq!(m.notices, [Notice::Info("压缩未执行：nothing to compact".into())]);

		notify(&mut m, json!({"type":"thinking_level_changed","thinkingLevel":"low"}));
		assert_eq!(m.state.as_ref().unwrap().thinking_level, Some(omp_rpc::ThinkingLevel::Low));
		notify(&mut m, json!({"type":"session_info_update","sessionId":"s","title":"新标题"}));
		assert_eq!(m.state.as_ref().unwrap().session_name.as_deref(), Some("新标题"));
		notify(&mut m, json!({"type":"todo_auto_clear"}));
		assert!(m.state.as_ref().unwrap().todo_phases.is_empty());
		assert!(notify(&mut m, json!({"type":"model_changed"})).refresh_state);
	}
}
