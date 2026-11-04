//! One card per tool call: header (tool, main argument, status), then a
//! tool-specific body built from the live `ToolRun`.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::*;
use omp_store::display::ToolArgs;
use omp_store::tools::{Diff, LineKind, Status, ToolRun};
use serde_json::{Map, Value};

use crate::copy;
use crate::theme::Palette;

/// Lines of bash output or read content shown before "show more".
const PREVIEW_LINES: usize = 14;
/// Diff lines shown before "show more".
const DIFF_LINES: usize = 40;

pub struct ToolCard<'a> {
	pub id:       usize,
	pub name:     &'a str,
	pub args:     &'a ToolArgs,
	pub intent:   Option<&'a str>,
	pub run:      Option<&'a ToolRun>,
	pub expanded: bool,
}

impl ToolCard<'_> {
	/// The argument that identifies the call: a command, a path…
	fn headline(&self) -> String {
		let ToolArgs::Complete(args) = self.args else {
			return String::new();
		};
		let string = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
		match self.name {
			"bash" => string("command").map(|c| format!("$ {c}")),
			"read" | "write" | "grep" | "glob" | "find" => string("path").or_else(|| string("pattern")),
			"edit" => self.run.and_then(|run| run.details.get("path")?.as_str().map(str::to_owned)).or_else(|| edit_target(args)),
			_ => None,
		}
		.unwrap_or_default()
	}

	fn status(&self, p: &Palette) -> (String, u32) {
		match self.run {
			None if matches!(self.args, ToolArgs::Partial(_)) => ("准备中".into(), p.warning),
			None => ("等待执行".into(), p.muted_foreground),
			Some(run) => {
				let time = run.elapsed.map(format_duration).unwrap_or_default();
				match run.status {
					Status::Running => (format!("运行中 {}", format_duration(run.started.elapsed())), p.primary),
					Status::Done => (time, p.success),
					Status::Failed => (format!("失败 {time}"), p.danger),
				}
			},
		}
	}

	/// The card, plus whether a "show more" toggle applies.
	pub fn render(self, p: &Palette, on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> AnyElement {
		let (status, status_color) = self.status(p);
		let failed = self.run.is_some_and(|run| run.status == Status::Failed);
		let diff = (self.name == "edit").then(|| self.run.and_then(ToolRun::diff)).flatten();
		let headline = self.headline();
		let headline = if headline.is_empty() { self.intent.unwrap_or_default().to_owned() } else { headline };
		let (headline, hidden_headline) = clip_lines(&headline, if self.expanded { usize::MAX } else { PREVIEW_LINES });

		let mut header = div()
			.flex()
			.items_start()
			.gap_2()
			.px_3()
			.py_1p5()
			.min_h(px(30.))
			.text_xs()
			.child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(rgb(p.accent_foreground)).child(self.name.to_owned()))
			.child(
				div()
					.flex_1()
					.min_w_0()
					.font_family("Menlo")
					.child(headline),
			);
		if let Some(diff) = &diff {
			header = header
				.child(div().flex_none().text_color(rgb(p.success)).child(format!("+{}", diff.added)))
				.child(div().flex_none().text_color(rgb(p.danger)).child(format!("-{}", diff.removed)));
		}
		header = header.child(div().flex_none().text_color(rgb(status_color)).child(status));
		// Copies the command itself (no `$ ` prompt) or the main argument.
		let copy_arg = match (self.name, self.args) {
			("bash", ToolArgs::Complete(args)) => args.get("command").and_then(Value::as_str).map(str::to_owned),
			_ => Some(self.headline()),
		}
		.filter(|text| !text.trim().is_empty());
		if let Some(text) = copy_arg {
			let tooltip = if self.name == "bash" { "复制命令" } else { "复制" };
			header = header.child(
				div()
					.flex_none()
					.invisible()
					.group_hover(copy::HOVER_GROUP, |s| s.visible())
					.child(copy::button(("tool-copy-arg", self.id), text, tooltip)),
			);
		}

		let (body, hidden_body) = match (self.name, self.run, &diff) {
			("edit", _, Some(diff)) => {
				let (body, hidden) = render_diff(diff, p, self.expanded);
				(Some(body), hidden)
			},
			// A successful read is one line; its content is in the file.
			("read", Some(run), _) if run.status == Status::Done => (None, 0),
			(_, Some(run), _) if !run.output.trim().is_empty() => {
				let (text, hidden) = clip_lines(&run.output, if self.expanded { usize::MAX } else { PREVIEW_LINES });
				let body = div()
					.relative()
					.px_3()
					.py_2()
					.border_t_1()
					.border_color(rgb(p.border))
					.bg(rgb(p.code))
					.font_family("Menlo")
					.text_xs()
					.text_color(if failed { rgb(p.danger) } else { rgb(p.foreground) })
					.child(text)
					// The whole output, including lines folded away.
					.child(
						div()
							.absolute()
							.top_1()
							.right_1()
							.invisible()
							.group_hover(copy::HOVER_GROUP, |s| s.visible())
							.child(copy::button(("tool-copy-output", self.id), run.output.clone(), "复制输出")),
					)
					.into_any_element();
				(Some(body), hidden)
			},
			(_, None, _) => {
				let args = match self.args {
					ToolArgs::Complete(args) => summarize_args(args),
					ToolArgs::Partial(raw) => raw.clone(),
				};
				let body = (!args.is_empty()).then(|| {
					div()
						.px_3()
						.pb_2()
						.font_family("Menlo")
						.text_xs()
						.text_color(rgb(p.muted_foreground))
						.child(args)
						.into_any_element()
				});
				(body, 0)
			},
			_ => (None, 0),
		};
		let hidden = hidden_headline + hidden_body;

		div()
			.id(("tool-card", self.id))
			.group(copy::HOVER_GROUP)
			.rounded(px(8.))
			.border_1()
			.border_color(if failed { rgb(p.danger) } else { rgb(p.border) })
			.bg(rgb(p.elevated))
			.overflow_hidden()
			.child(header)
			.children(body)
			.when(hidden > 0 || self.expanded, |card| {
				card.child(
					div()
						.id(("tool-card-more", self.id))
						.px_3()
						.py_1()
						.border_t_1()
						.border_color(rgb(p.border))
						.text_xs()
						.text_color(rgb(p.muted_foreground))
						.cursor_pointer()
						.child(if self.expanded { "收起".to_owned() } else { format!("展开其余 {hidden} 行") })
						.on_click(on_toggle)
						.test_support(),
				)
			})
			.test_support()
			.into_any_element()
	}
}

fn render_diff(diff: &Diff, p: &Palette, expanded: bool) -> (AnyElement, usize) {
	let limit = if expanded { usize::MAX } else { DIFF_LINES };
	let hidden = diff.lines.len().saturating_sub(limit);
	let rows = diff.lines.iter().take(limit).map(|line| {
		let (bg, fg, sign) = match line.kind {
			LineKind::Added => (Some(diff_bg(p, true)), p.success, "+"),
			LineKind::Removed => (Some(diff_bg(p, false)), p.danger, "-"),
			LineKind::Context => (None, p.foreground, " "),
			LineKind::Gap => (None, p.muted_foreground, ""),
		};
		div()
			.flex()
			.when_some(bg, |row, bg| row.bg(rgb(bg)))
			.child(
				div()
					.w(px(36.))
					.flex_none()
					.pr_2()
					.text_right()
					.text_color(rgb(p.muted_foreground))
					.child(line.number.map(|n| n.to_string()).unwrap_or_default()),
			)
			.child(div().w(px(14.)).flex_none().text_color(rgb(fg)).child(sign))
			.child(div().flex_1().whitespace_nowrap().text_color(rgb(fg)).child(line.text.clone()))
	});
	let body = div()
		.border_t_1()
		.border_color(rgb(p.border))
		.bg(rgb(p.code))
		.py_1()
		.font_family("Menlo")
		.text_xs()
		.children(rows)
		.into_any_element();
	(body, hidden)
}

fn diff_bg(p: &Palette, added: bool) -> u32 {
	let dark = p.background == crate::theme::DARK.background;
	match (added, dark) {
		(true, false) => 0xE6F6EC,
		(true, true) => 0x122B1E,
		(false, false) => 0xFCECEB,
		(false, true) => 0x301619,
	}
}

/// The file an edit targets, from its `[path#TAG]` header.
fn edit_target(args: &Map<String, Value>) -> Option<String> {
	let input = args.get("input")?.as_str()?;
	let header = input.lines().next()?.trim().strip_prefix('[')?.strip_suffix(']')?;
	Some(header.rsplit_once('#').map_or(header, |(path, _)| path).to_owned())
}

fn summarize_args(args: &Map<String, Value>) -> String {
	args.iter()
		.filter(|(key, _)| key.as_str() != "i")
		.map(|(key, value)| {
			let value = match value {
				Value::String(s) => s.clone(),
				other => other.to_string(),
			};
			format!("{key}: {}", truncate(&value, 200))
		})
		.collect::<Vec<_>>()
		.join("\n")
}

pub fn truncate(text: &str, max: usize) -> String {
	let mut chars = text.chars();
	let head: String = chars.by_ref().take(max).collect();
	if chars.next().is_some() { format!("{head}…") } else { head }
}

/// The first `limit` lines and how many were left out.
pub fn clip_lines(text: &str, limit: usize) -> (String, usize) {
	let lines: Vec<&str> = text.trim_end().lines().collect();
	let shown = lines.len().min(limit);
	(lines[..shown].join("\n"), lines.len() - shown)
}

fn format_duration(duration: std::time::Duration) -> String {
	let ms = duration.as_millis();
	if ms < 1000 { format!("{ms}ms") } else { format!("{:.1}s", duration.as_secs_f64()) }
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;
	use ::core::prelude::v1::test;

	#[::core::prelude::v1::test]
	fn edit_target_from_hashline_header() {
		let args = json!({"input": "[src/lib.rs#A1B2]\nPUT 1.=1:\n+x"});
		assert_eq!(edit_target(args.as_object().unwrap()).as_deref(), Some("src/lib.rs"));
		let args = json!({"input": "no header"});
		assert_eq!(edit_target(args.as_object().unwrap()), None);
	}

	#[::core::prelude::v1::test]
	fn clip_counts_hidden_lines() {
		assert_eq!(clip_lines("a\nb\nc\n", 2), ("a\nb".to_owned(), 1));
		assert_eq!(clip_lines("a\n", 5), ("a".to_owned(), 0));
	}

	#[gpui_kit::test]
	async fn multiline_bash_header_grows_and_clips_long_scripts(cx: &mut TestAppContext) {
		use gpui_kit::test::TestWindowExt as _;

		cx.update(gpui_kit::init);
		cx.update(crate::theme::apply);

		struct Harness;
		impl Render for Harness {
			fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
				let p = crate::theme::palette(cx);
				let args = |cmd: &str| ToolArgs::Complete(json!({"command": cmd}).as_object().unwrap().clone());
				let run = ToolRun {
					name:    "bash".into(),
					status:  Status::Done,
					output:  "patched: 63\n".into(),
					details: Value::Null,
					started: std::time::Instant::now(),
					elapsed: Some(std::time::Duration::from_millis(120)),
				};
				let single = args("echo hi");
				let multi = args("cd ~/.omp/agent && python3 << 'EOF'\nimport pathlib, re\n\nMOVED = [\"a\", \"b\"]\nfor r in MOVED:\n    print(r)\nEOF");
				let twenty_lines = (1..=20).map(|i| format!("echo line {i}")).collect::<Vec<_>>().join("\n");
				let long_script = args(&twenty_lines);
				div()
					.w(px(600.))
					.flex()
					.flex_col()
					.gap_2()
					.child(ToolCard { id: 1, name: "bash", args: &single, intent: None, run: Some(&run), expanded: false }.render(p, |_, _, _| {}))
					.child(ToolCard { id: 2, name: "bash", args: &multi, intent: None, run: Some(&run), expanded: false }.render(p, |_, _, _| {}))
					.child(ToolCard { id: 3, name: "bash", args: &long_script, intent: None, run: Some(&run), expanded: false }.render(p, |_, _, _| {}))
			}
		}

		let (handle, _) = cx.update(|cx| gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Harness)).unwrap());
		cx.update_window(handle, |_, window, cx| {
			window.render_frame(cx);
			let h1 = window.find(("tool-card", 1usize)).bounds().size.height;
			let h2 = window.find(("tool-card", 2usize)).bounds().size.height;
			assert!(h2 > h1 + px(60.), "multiline bash command should expand header height (single={h1:?}, multi={h2:?})");
			assert!(window.try_find(("tool-card-more", 2usize)).is_none(), "7-line command fits within PREVIEW_LINES");
			assert!(window.try_find(("tool-card-more", 3usize)).is_some(), "20-line command shows expand toggle");

			// Copy buttons: the bare command (no `$ `) and the full output.
			window.hover(("tool-card", 2usize), cx);
			window.click(("tool-copy-arg", 2usize), cx);
			let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
			assert!(copied.starts_with("cd ~/.omp/agent && python3"), "{copied:?}");
			window.hover(("tool-card", 1usize), cx);
			window.click(("tool-copy-output", 1usize), cx);
			assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("patched: 63\n"));
		})
		.unwrap();
	}
}
