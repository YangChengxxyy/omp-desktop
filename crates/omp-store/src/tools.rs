//! Tool execution state keyed by tool-call id, folded from
//! `tool_execution_*` events and `toolResult` messages, so a view can render
//! each call as one card: arguments, live output, final result, duration.
//!
//! Also parses the `edit` tool's `details.diff` into display lines.

use std::collections::HashMap;
use std::time::Instant;

use omp_rpc::{AgentMessage, RpcAgentEvent, UserContent};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
	Running,
	Done,
	Failed,
}

#[derive(Debug, Clone)]
pub struct ToolRun {
	pub name:    String,
	pub status:  Status,
	/// Text output: streamed partial output while running, final content after.
	pub output:  String,
	/// The result's `details` object (tool specific: `diff`, `wallTimeMs`…).
	pub details: Value,
	pub started: Instant,
	/// Wall time reported by the tool (`details.wallTimeMs`) or measured here.
	pub elapsed: Option<std::time::Duration>,
}

impl ToolRun {
	pub fn diff(&self) -> Option<Diff> {
		self.details.get("diff").and_then(Value::as_str).map(Diff::parse)
	}
}

#[derive(Debug, Default)]
pub struct ToolRuns {
	runs: HashMap<String, ToolRun>,
}

impl ToolRuns {
	pub fn get(&self, tool_call_id: &str) -> Option<&ToolRun> {
		self.runs.get(tool_call_id)
	}

	pub fn iter(&self) -> impl Iterator<Item = (&str, &ToolRun)> {
		self.runs.iter().map(|(id, run)| (id.as_str(), run))
	}

	/// Folds one event; returns whether a run changed.
	pub fn apply(&mut self, event: &RpcAgentEvent) -> bool {
		match event {
			RpcAgentEvent::ToolExecutionStart(start) => {
				self.runs.insert(start.tool_call_id.clone(), ToolRun {
					name:    start.tool_name.clone(),
					status:  Status::Running,
					output:  String::new(),
					details: Value::Null,
					started: Instant::now(),
					elapsed: None,
				});
				true
			},
			RpcAgentEvent::ToolExecutionUpdate(update) => {
				let Some(run) = self.runs.get_mut(&update.tool_call_id) else {
					return false;
				};
				if let Some(partial) = &update.partial_result {
					run.output = content_text(partial.get("content"));
				}
				true
			},
			RpcAgentEvent::ToolExecutionEnd(end) => {
				let run = self.runs.entry(end.tool_call_id.clone()).or_insert_with(|| ToolRun {
					name:    end.tool_name.clone(),
					status:  Status::Running,
					output:  String::new(),
					details: Value::Null,
					started: Instant::now(),
					elapsed: None,
				});
				let result = end.result.clone().unwrap_or(Value::Null);
				run.finish(content_text(result.get("content")), result.get("details").cloned(), end.is_error == Some(true));
				true
			},
			// History loaded after the fact has results without execution events.
			RpcAgentEvent::MessageEnd(end) => {
				let AgentMessage::ToolResult(result) = &end.message else {
					return false;
				};
				let Some(id) = &result.tool_call_id else {
					return false;
				};
				if self.runs.get(id).is_some_and(|run| run.status != Status::Running) {
					return false;
				}
				let output = result
					.content
					.iter()
					.flatten()
					.filter_map(|part| match part {
						UserContent::Text(text) => text.text.clone(),
						UserContent::Image(_) => None,
					})
					.collect::<Vec<_>>()
					.join("\n");
				let run = self.runs.entry(id.clone()).or_insert_with(|| ToolRun {
					name:    result.tool_name.clone().unwrap_or_default(),
					status:  Status::Running,
					output:  String::new(),
					details: Value::Null,
					started: Instant::now(),
					elapsed: None,
				});
				run.finish(output, result.details.clone(), result.is_error == Some(true));
				true
			},
			_ => false,
		}
	}
}

impl ToolRun {
	fn finish(&mut self, output: String, details: Option<Value>, failed: bool) {
		self.output = output;
		self.details = details.unwrap_or(Value::Null);
		self.status = if failed { Status::Failed } else { Status::Done };
		self.elapsed = Some(
			self.details
				.get("wallTimeMs")
				.and_then(Value::as_f64)
				.map(|ms| std::time::Duration::from_secs_f64(ms / 1000.))
				.unwrap_or_else(|| self.started.elapsed()),
		);
	}
}

fn content_text(content: Option<&Value>) -> String {
	content
		.and_then(Value::as_array)
		.into_iter()
		.flatten()
		.filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
		.filter_map(|part| part.get("text").and_then(Value::as_str))
		.collect::<Vec<_>>()
		.join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
	Context,
	Added,
	Removed,
	/// A `...` gap between hunks.
	Gap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
	pub kind:   LineKind,
	pub number: Option<u32>,
	pub text:   String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
	pub lines:   Vec<DiffLine>,
	pub added:   usize,
	pub removed: usize,
}

impl Diff {
	/// Parses omp's edit diff: one line per row, `<sign><lineno>|<text>`
	/// where sign is `+`, `-`, or a space. Rows that do not fit the shape
	/// (hunk separators) become gaps.
	pub fn parse(text: &str) -> Self {
		let mut diff = Diff::default();
		for row in text.lines() {
			let mut chars = row.chars();
			let kind = match chars.next() {
				Some('+') => LineKind::Added,
				Some('-') => LineKind::Removed,
				Some(' ') => LineKind::Context,
				_ => {
					diff.lines.push(DiffLine { kind: LineKind::Gap, number: None, text: row.to_owned() });
					continue;
				},
			};
			let rest = chars.as_str();
			let Some((number, body)) = rest.split_once('|') else {
				diff.lines.push(DiffLine { kind: LineKind::Gap, number: None, text: row.to_owned() });
				continue;
			};
			let Ok(number) = number.trim().parse::<u32>() else {
				diff.lines.push(DiffLine { kind: LineKind::Gap, number: None, text: row.to_owned() });
				continue;
			};
			match kind {
				LineKind::Added => diff.added += 1,
				LineKind::Removed => diff.removed += 1,
				_ => {},
			}
			diff.lines.push(DiffLine { kind, number: Some(number), text: body.to_owned() });
		}
		diff
	}
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	fn event(value: Value) -> RpcAgentEvent {
		RpcAgentEvent::from_value(value).expect("decodes")
	}

	// Frames below are trimmed copies of a real omp 18.13.3 session
	// (bash `echo hello && ls`, then an edit renaming `a` to `b`).
	#[test]
	fn bash_streams_partial_output_then_finishes_with_wall_time() {
		let mut runs = ToolRuns::default();
		runs.apply(&event(json!({"type":"tool_execution_start","toolCallId":"c1","toolName":"bash",
			"args":{"command":"echo hello && ls"}})));
		assert_eq!(runs.get("c1").unwrap().status, Status::Running);
		runs.apply(&event(json!({"type":"tool_execution_update","toolCallId":"c1","toolName":"bash",
			"args":{},"partialResult":{"content":[{"type":"text","text":"hello\n"}],"details":{}}})));
		assert_eq!(runs.get("c1").unwrap().output, "hello\n");
		runs.apply(&event(json!({"type":"tool_execution_end","toolCallId":"c1","toolName":"bash",
			"result":{"content":[{"type":"text","text":"hello\ndemo.rs\n"}],"details":{"wallTimeMs":172.6}},"isError":false})));
		let run = runs.get("c1").unwrap();
		assert_eq!(run.status, Status::Done);
		assert_eq!(run.output, "hello\ndemo.rs\n");
		assert_eq!(run.elapsed.unwrap().as_millis(), 172);
	}

	#[test]
	fn failed_end_marks_run_failed() {
		let mut runs = ToolRuns::default();
		runs.apply(&event(json!({"type":"tool_execution_start","toolCallId":"c","toolName":"read","args":{}})));
		runs.apply(&event(json!({"type":"tool_execution_end","toolCallId":"c","toolName":"read",
			"result":{"content":[{"type":"text","text":"ENOENT"}]},"isError":true})));
		assert_eq!(runs.get("c").unwrap().status, Status::Failed);
	}

	#[test]
	fn tool_result_message_fills_runs_without_execution_events() {
		let mut runs = ToolRuns::default();
		runs.apply(&event(json!({"type":"message_end","messageId":"r","message":{"role":"toolResult",
			"toolCallId":"c9","toolName":"read","content":[{"type":"text","text":"body"}],"isError":false}})));
		let run = runs.get("c9").unwrap();
		assert_eq!((run.name.as_str(), run.status.clone(), run.output.as_str()), ("read", Status::Done, "body"));
	}

	#[test]
	fn parses_edit_diff_with_counts_and_gaps() {
		let diff = Diff::parse(" 1|fn main() {\n-2|    let a = 1;\n-3|    println!(\"{}\", a);\n+2|    let b = 1;\n+3|    println!(\"{}\", b);\n 4|}\n...\n 40|tail");
		assert_eq!((diff.added, diff.removed), (2, 2));
		assert_eq!(diff.lines[1], DiffLine { kind: LineKind::Removed, number: Some(2), text: "    let a = 1;".into() });
		assert_eq!(diff.lines[3].kind, LineKind::Added);
		assert_eq!(diff.lines[6], DiffLine { kind: LineKind::Gap, number: None, text: "...".into() });
		assert_eq!(diff.lines[7], DiffLine { kind: LineKind::Context, number: Some(40), text: "tail".into() });
	}

	#[test]
	fn pipe_inside_code_stays_in_text() {
		let diff = Diff::parse("+7|let x = a || b;");
		assert_eq!(diff.lines[0].text, "let x = a || b;");
	}
}
