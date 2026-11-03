//! Text for the status bar and the detail panel, derived from `get_state`
//! and `get_session_stats`. Free of GPUI so it can be tested directly.

use omp_rpc::{SessionState, TodoPhase, TodoStatus, UsageLimitState};

/// `84k`, `1.2M`: token counts short enough for the status bar.
pub fn tokens(n: i64) -> String {
	match n {
		..1_000 => n.to_string(),
		1_000..1_000_000 => format!("{}k", (n as f64 / 1_000.).round() as i64),
		_ => {
			let m = n as f64 / 1_000_000.;
			if (m - m.round()).abs() < 0.05 { format!("{}M", m.round() as i64) } else { format!("{m:.1}M") }
		},
	}
}

/// (completed, total) over every phase.
pub fn todo_counts(phases: &[TodoPhase]) -> (usize, usize) {
	let tasks = phases.iter().flat_map(|phase| &phase.tasks);
	let total = tasks.clone().filter(|task| task.status != TodoStatus::Abandoned).count();
	let done = tasks.filter(|task| task.status == TodoStatus::Completed).count();
	(done, total)
}

pub fn todo_mark(status: TodoStatus) -> &'static str {
	match status {
		TodoStatus::Pending => "☐",
		TodoStatus::InProgress => "◐",
		TodoStatus::Completed => "☑",
		TodoStatus::Abandoned => "✕",
		TodoStatus::Blocked => "⛔",
	}
}

/// Context use above which the bar turns amber, then red.
pub const CONTEXT_WARN: f64 = 80.;
pub const CONTEXT_DANGER: f64 = 95.;

fn yes_no(value: bool) -> String {
	(if value { "是" } else { "否" }).into()
}

fn on_off(value: bool) -> String {
	(if value { "开" } else { "关" }).into()
}

fn or_dash(value: Option<String>) -> String {
	value.filter(|v| !v.is_empty()).unwrap_or_else(|| "—".into())
}

/// One row per `get_state` field, keyed by its wire name, so the panel shows
/// every field even when it is unset.
pub fn state_rows(state: &SessionState) -> Vec<(&'static str, &'static str, String)> {
	let queued = &state.queued_messages;
	let (done, total) = todo_counts(&state.todo_phases);
	let prompt_chars: usize = state.system_prompt.iter().map(|s| s.chars().count()).sum();
	vec![
		("sessionId", "会话 ID", state.session_id.clone()),
		("sessionName", "名称", or_dash(state.session_name.clone())),
		("sessionFile", "文件", or_dash(state.session_file.clone())),
		(
			"model",
			"模型",
			or_dash(state.model.as_ref().map(|m| {
				let window = m.context_window.map(|w| format!(" · {}", tokens(w))).unwrap_or_default();
				format!("{}/{}{window}", m.provider, m.id)
			})),
		),
		("thinkingLevel", "思考档位", or_dash(state.thinking_level.map(|t| t.as_str().to_owned()))),
		(
			"contextUsage",
			"上下文",
			or_dash(state.context_usage.as_ref().map(|u| {
				format!("{} / {}（{:.0}%）", tokens(u.tokens), tokens(u.context_window), u.percent)
			})),
		),
		("autoCompactionEnabled", "自动压缩", on_off(state.auto_compaction_enabled)),
		("isStreaming", "输出中", yes_no(state.is_streaming)),
		("isCompacting", "压缩中", yes_no(state.is_compacting)),
		("isSettled", "已静止", yes_no(state.is_settled)),
		("hasPendingAsyncWork", "后台任务", yes_no(state.has_pending_async_work)),
		("messageCount", "消息数", state.message_count.to_string()),
		("queuedMessageCount", "排队数", state.queued_message_count.to_string()),
		(
			"queuedMessages",
			"队列",
			format!("steer {} · follow-up {} · 已注入 {}", queued.steering.len(), queued.follow_up.len(), queued.live_steered),
		),
		("steeringMode", "steer 模式", state.steering_mode.as_str().into()),
		("followUpMode", "follow-up 模式", state.follow_up_mode.as_str().into()),
		("interruptMode", "打断模式", state.interrupt_mode.as_str().into()),
		("fastModeEnabled", "快速模式", on_off(state.fast_mode_enabled)),
		("fastModeActive", "快速模式生效", yes_no(state.fast_mode_active)),
		("slowModeSupported", "支持慢速模式", yes_no(state.slow_mode_supported)),
		("slowModeEnabled", "慢速模式", on_off(state.slow_mode_enabled)),
		("slowModeScope", "慢速模式范围", or_dash(state.slow_mode_scope.map(|s| s.as_str().to_owned()))),
		(
			"usageLimit",
			"用量限制",
			or_dash(state.usage_limit.as_ref().map(|limit| match limit {
				UsageLimitState::LowPriority(low) => match low.allowance_left_percent {
					Some(left) => format!("低优先级通道，剩余 {left}%"),
					None => "低优先级通道".into(),
				},
				UsageLimitState::WrapUp(wrap) => {
					if wrap.extra_usage { "收尾额度（之后按量计费）".into() } else { "收尾额度".into() }
				},
			})),
		),
		("tokensPerSecond", "输出速度", or_dash(state.tokens_per_second.map(|t| format!("{t:.1} tok/s")))),
		("todoPhases", "Todo", if total == 0 { "—".into() } else { format!("{done}/{total}") }),
		("systemPrompt", "系统提示", format!("{} 段 · {prompt_chars} 字", state.system_prompt.len())),
		("dumpTools", "工具", format!("{} 个", state.dump_tools.len())),
		(
			"goal",
			"目标",
			or_dash(state.goal.as_ref().map(|g| format!("{}（{}）", g.goal.objective, g.goal.status.as_str()))),
		),
	]
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	#[test]
	fn token_counts_shorten() {
		assert_eq!(tokens(950), "950");
		assert_eq!(tokens(84_200), "84k");
		assert_eq!(tokens(1_000_000), "1M");
		assert_eq!(tokens(1_048_576), "1M");
		assert_eq!(tokens(1_250_000), "1.2M");
	}

	#[test]
	fn todo_counts_skip_abandoned() {
		let phases: Vec<TodoPhase> = serde_json::from_value(json!([
			{"name":"a","tasks":[{"content":"1","status":"completed"},{"content":"2","status":"in_progress"}]},
			{"name":"b","tasks":[{"content":"3","status":"abandoned"},{"content":"4","status":"pending"}]},
		]))
		.unwrap();
		assert_eq!(todo_counts(&phases), (1, 3));
	}

	/// Every field `get_state` can send has a row, including the ones the
	/// server omits when unset.
	#[test]
	fn state_rows_cover_every_wire_field() {
		let state: SessionState = serde_json::from_value(json!({
			"sessionId":"s","thinkingLevel":"high","isStreaming":false,"isCompacting":false,
			"steeringMode":"one-at-a-time","followUpMode":"all","interruptMode":"immediate",
			"autoCompactionEnabled":true,"fastModeEnabled":false,"fastModeActive":false,
			"slowModeSupported":true,"slowModeEnabled":true,"slowModeScope":"session",
			"usageLimit":{"stage":"low_priority","resetsAtSec":1.0,"allowanceLeftPercent":30},
			"tokensPerSecond":42.0,"messageCount":3,"queuedMessageCount":0,"hasPendingAsyncWork":false,
			"isSettled":true,"queuedMessages":{"steering":[],"followUp":[],"liveSteered":0},"todoPhases":[],
			"systemPrompt":["abc"],"dumpTools":[],"contextUsage":{"tokens":84000,"contextWindow":200000,"percent":42.0},
			"sessionFile":"/f.jsonl","sessionName":"n","goal":null
		}))
		.unwrap();
		let mut wire: std::collections::BTreeSet<String> =
			serde_json::to_value(&state).unwrap().as_object().unwrap().keys().cloned().collect();
		// Unset optional fields are omitted on the wire.
		wire.extend(["model", "goal"].map(String::from));
		let rows = state_rows(&state);
		for key in &wire {
			assert!(rows.iter().any(|(k, ..)| k == key), "no row for {key}");
		}
		assert_eq!(rows.len(), wire.len(), "a row has no wire field");
		let value = |key: &str| rows.iter().find(|(k, ..)| *k == key).unwrap().2.clone();
		assert_eq!(value("contextUsage"), "84k / 200k（42%）");
		assert_eq!(value("usageLimit"), "低优先级通道，剩余 30%");
		assert_eq!(value("model"), "—");
	}
}
