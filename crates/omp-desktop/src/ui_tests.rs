//! Headless UI test: the real `ChatView` against a real `omp --mode rpc-ui`.
//! It calls a model, so it is ignored by default:
//!
//! cargo test -p omp-desktop -- --ignored --nocapture

use std::time::{Duration, Instant};

use gpui_kit::test::{TestInput, TestWindowExt as _};
use gpui_kit::{
	AppContext as _, Bounds, ElementInputHandler, Point, TestAppContext, WindowBounds, WindowOptions, px, size,
};
use omp_bridge::SessionOptions;
use omp_store::display::{Block, Role};

use crate::chat::ChatView;

#[gpui_kit::test]
#[ignore = "spawns omp and calls a model"]
async fn sends_prompt_and_renders_streamed_reply(cx: &mut TestAppContext) {
	// The session runs on real threads and pipes, outside the test scheduler.
	cx.executor().allow_parking();
	cx.update(gpui_kit::init);
	cx.update(crate::theme::apply);

	let options = SessionOptions {
		cwd: Some(std::env::temp_dir()),
		extra_args: ["--no-session", "--no-tools", "--thinking", "off"].map(String::from).to_vec(),
		..Default::default()
	};
	let (handle, view) = cx.update(|cx| {
		gpui_kit::open_window(
			WindowOptions {
				window_bounds: Some(WindowBounds::Windowed(Bounds {
					origin: Point::default(),
					size:   size(px(1000.), px(700.)),
				})),
				..Default::default()
			},
			cx,
			|window, cx| cx.new(|cx| ChatView::new(options, window, cx)),
		)
		.expect("open window")
	});

	wait_until(cx, Duration::from_secs(60), "omp connects", |cx| view.read_with(cx, |v, _| v.is_connected()));

	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.input("只回复：你好，桌面端", cx);
		window.press("enter", cx);
	})
	.unwrap();

	wait_until(cx, Duration::from_secs(120), "reply settles", |cx| {
		view.read_with(cx, |v, _| v.is_idle() && v.assistant_text().is_some())
	});

	let (user, reply, composer) = view.read_with(cx, |v, cx| (v.user_texts(), v.assistant_text(), v.composer_text(cx)));
	assert_eq!(user, ["只回复：你好，桌面端"]);
	let reply = reply.expect("assistant reply");
	assert!(reply.contains("你好"), "unexpected reply: {reply:?}");
	assert_eq!(composer, "", "composer clears after sending");

	// Rendering the finished transcript must not panic.
	cx.update_window(handle, |_, window, cx| window.render_frame(cx)).unwrap();
}

#[gpui_kit::test]
#[ignore = "spawns omp and calls a model"]
async fn approval_dialog_round_trip_lets_the_write_run(cx: &mut TestAppContext) {
	cx.executor().allow_parking();
	cx.update(gpui_kit::init);
	cx.update(crate::theme::apply);

	let dir = std::env::temp_dir().join(format!("omp-desktop-approval-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let target = dir.join("approved.txt");
	_ = std::fs::remove_file(&target);
	let options = SessionOptions {
		cwd: Some(dir.clone()),
		// `always-ask` prompts for every `write`-tier call, so the dialog must
		// round-trip before the file can exist.
		extra_args: ["--no-session", "--tools", "write", "--thinking", "off", "--approval-mode", "always-ask"]
			.map(String::from)
			.to_vec(),
		..Default::default()
	};
	let (handle, view) = cx.update(|cx| {
		gpui_kit::open_window(
			WindowOptions {
				window_bounds: Some(WindowBounds::Windowed(Bounds { origin: Point::default(), size: size(px(1000.), px(700.)) })),
				..Default::default()
			},
			cx,
			|window, cx| cx.new(|cx| ChatView::new(options, window, cx)),
		)
		.expect("open window")
	});
	wait_until(cx, Duration::from_secs(60), "omp connects", |cx| view.read_with(cx, |v, _| v.is_connected()));

	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.input("用 write 工具在当前目录创建 approved.txt，内容为 ok。不要做别的。", cx);
		window.press("enter", cx);
	})
	.unwrap();

	wait_until(cx, Duration::from_secs(120), "approval dialog opens", |cx| {
		view.read_with(cx, |v, cx| v.open_dialog_options(cx).is_some())
	});
	assert!(!target.exists(), "write ran before approval");
	let options = view.read_with(cx, |v, cx| v.open_dialog_options(cx)).unwrap();
	// Only a one-time answer: "Always …" choices persist a grant in the user's
	// real `approval-cache.db`.
	println!("approval options: {options:?}");
	let approve = options
		.iter()
		.position(|label| label == "Approve Once" || label == "Approve")
		.unwrap_or_else(|| panic!("no one-time approve option in {options:?}"));
	cx.update_window(handle, |_, window, cx| window.click(("select-option", approve), cx)).unwrap();

	wait_until(cx, Duration::from_secs(120), "turn settles", |cx| view.read_with(cx, |v, _| v.is_idle()));
	assert_eq!(std::fs::read_to_string(&target).ok().as_deref().map(str::trim), Some("ok"));
	assert!(view.read_with(cx, |v, cx| v.open_dialog_options(cx).is_none()), "dialog closes after answering");
	_ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
#[ignore = "spawns omp and calls a model"]
async fn ask_dialog_returns_multi_select_and_free_text(cx: &mut TestAppContext) {
	let (handle, view, dir) = open_chat(cx, "ask", &["--tools", "ask", "--approval-mode", "yolo"]);
	type_and(
		cx,
		handle,
		"Use the ask tool exactly once: one multi-select question with id \"fruit\", options Apple, Banana, Cherry. \
		 Then reply with exactly the selected options and any custom text I typed, comma separated, nothing else.",
		"enter",
	);
	wait_until(cx, Duration::from_secs(120), "ask dialog opens", |cx| {
		view.read_with(cx, |v, cx| v.open_ask_labels(cx).is_some())
	});
	let labels = view.read_with(cx, |v, cx| v.open_ask_labels(cx)).unwrap();
	println!("ask options: {labels:?}");
	let index = |name: &str| labels.iter().position(|l| l == name).unwrap_or_else(|| panic!("no {name} in {labels:?}"));
	let (apple, cherry) = (index("Apple"), index("Cherry"));
	cx.update_window(handle, |_, window, cx| {
		window.click(("ask-option", apple), cx);
		window.click(("ask-option", cherry), cx);
		window.click(("ask-custom", 0usize), cx);
		window.input("Durian", cx);
		window.click("dialog-submit", cx);
	})
	.unwrap();

	wait_until(cx, Duration::from_secs(120), "reply settles", |cx| {
		view.read_with(cx, |v, cx| v.is_idle() && v.open_ask_labels(cx).is_none() && v.assistant_text().is_some())
	});
	let reply = view.read_with(cx, |v, _| v.assistant_text()).unwrap();
	for expected in ["Apple", "Cherry", "Durian"] {
		assert!(reply.contains(expected), "{expected} missing from reply {reply:?}");
	}
	assert!(!reply.contains("Banana"), "unselected option in reply {reply:?}");
	_ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
#[ignore = "spawns omp and calls a model"]
async fn busy_enter_queues_chip_removal_and_esc_aborts(cx: &mut TestAppContext) {
	let (handle, view, dir) = open_chat(cx, "abort", &["--tools", "bash", "--approval-mode", "yolo"]);
	type_and(cx, handle, "Run this exact bash command and nothing else: sleep 60", "enter");
	wait_until(cx, Duration::from_secs(120), "bash starts", |cx| {
		view.read_with(cx, |v, _| {
			v.tool_runs_running("bash")
		})
	});

	// Busy: Enter queues a follow-up instead of starting a turn.
	type_and(cx, handle, "排队的追问", "enter");
	wait_until(cx, Duration::from_secs(20), "follow-up chip appears", |cx| {
		view.read_with(cx, |v, _| v.queue().follow_up == ["排队的追问"])
	});
	cx.update_window(handle, |_, window, cx| window.click(("queue-remove", 0usize), cx)).unwrap();
	wait_until(cx, Duration::from_secs(20), "chip removed", |cx| {
		view.read_with(cx, |v, _| v.queue().follow_up.is_empty() && v.queue().steering.is_empty())
	});

	// Esc that cancels a pinyin composition in an empty composer must not
	// abort. The composing text is already part of the composer's value when
	// the capture-phase handler sees Esc, so the non-empty check covers it.
	let composer = view.read_with(cx, |v, _| v.composer());
	cx.update_window(handle, |_, window, cx| {
		window.click("composer", cx);
		let bounds = window.find("composer").bounds();
		let mut ime = TestInput::new(ElementInputHandler::new(bounds, composer.clone()), window);
		ime.compose_text(None, "ni", None, cx);
		assert!(ime.marked_text_range(cx).is_some(), "composition is marked");
		window.press("escape", cx);
	})
	.unwrap();
	assert_eq!(view.read_with(cx, |v, _| v.aborts), 0, "Esc during IME composition sent an abort");
	// Clear whatever the cancelled composition left, so the composer is empty.
	cx.update_window(handle, |_, window, cx| {
		composer.update(cx, |state, cx| state.clean(window, cx));
		window.render_frame(cx);
	})
	.unwrap();
	assert_eq!(view.read_with(cx, |v, cx| v.composer_text(cx)), "");

	// Empty composer + Esc stops the run long before `sleep 60` ends, with
	// exactly one abort per key press.
	let stopped = Instant::now();
	cx.update_window(handle, |_, window, cx| window.press("escape", cx)).unwrap();
	assert_eq!(view.read_with(cx, |v, _| v.aborts), 1, "one Esc must send one abort");
	wait_until(cx, Duration::from_secs(30), "abort settles", |cx| view.read_with(cx, |v, _| v.is_idle()));
	assert!(stopped.elapsed() < Duration::from_secs(30), "abort took {:?}", stopped.elapsed());
	assert!(view.read_with(cx, |v, _| !v.tool_runs_running("bash")), "bash still marked running");
	// The removed follow-up never reached the transcript.
	assert_eq!(view.read_with(cx, |v, _| v.user_texts()), ["Run this exact bash command and nothing else: sleep 60"]);
	_ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
#[ignore = "spawns omp and calls a model"]
async fn edit_card_renders_parsed_diff(cx: &mut TestAppContext) {
	let (handle, view, dir) = open_chat(cx, "edit", &["--tools", "read,edit", "--approval-mode", "yolo"]);
	std::fs::write(dir.join("demo.rs"), "fn main() {\n    let a = 1;\n    println!(\"{}\", a);\n}\n").unwrap();
	type_and(cx, handle, "In demo.rs rename the variable a to b (both lines). Use read then edit. Nothing else.", "enter");
	wait_until(cx, Duration::from_secs(180), "turn settles", |cx| view.read_with(cx, |v, _| v.is_idle()));

	let diff = view.read_with(cx, |v, _| v.last_edit_diff()).expect("edit ran with a diff");
	assert_eq!((diff.added, diff.removed), (2, 2), "{diff:?}");
	assert!(std::fs::read_to_string(dir.join("demo.rs")).unwrap().contains("let b = 1;"));
	// The card with the diff renders without panicking and its expander toggles.
	cx.update_window(handle, |_, window, cx| window.render_frame(cx)).unwrap();
	_ = std::fs::remove_dir_all(&dir);
}

/// Opens the real `ChatView` on a fresh temp dir and waits for omp.
fn open_chat(
	cx: &mut TestAppContext,
	name: &str,
	args: &[&str],
) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<ChatView>, std::path::PathBuf) {
	cx.executor().allow_parking();
	cx.update(gpui_kit::init);
	cx.update(crate::theme::apply);
	let dir = std::env::temp_dir().join(format!("omp-desktop-{name}-{}", std::process::id()));
	_ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).unwrap();
	let mut extra_args: Vec<String> = ["--no-session", "--thinking", "off"].map(String::from).to_vec();
	extra_args.extend(args.iter().map(|a| a.to_string()));
	let options = SessionOptions { cwd: Some(dir.clone()), extra_args, ..Default::default() };
	let (handle, view) = cx.update(|cx| {
		gpui_kit::open_window(
			WindowOptions {
				window_bounds: Some(WindowBounds::Windowed(Bounds { origin: Point::default(), size: size(px(1000.), px(800.)) })),
				..Default::default()
			},
			cx,
			|window, cx| cx.new(|cx| ChatView::new(options, window, cx)),
		)
		.expect("open window")
	});
	wait_until(cx, Duration::from_secs(60), "omp connects", |cx| view.read_with(cx, |v, _| v.is_connected()));
	(handle, view, dir)
}

fn type_and(cx: &mut TestAppContext, handle: gpui_kit::AnyWindowHandle, text: &str, key: &str) {
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.input(text, cx);
		window.press(key, cx);
	})
	.unwrap();
}
fn wait_until(
	cx: &mut TestAppContext,
	timeout: Duration,
	what: &str,
	mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
	let deadline = Instant::now() + timeout;
	while !done(cx) {
		assert!(Instant::now() < deadline, "timed out waiting for: {what}");
		cx.run_until_parked();
		std::thread::sleep(Duration::from_millis(20));
	}
}

impl ChatView {
	fn rows(&self) -> Vec<omp_store::display::View> {
		self.transcript().entries().iter().map(omp_store::Entry::view).collect()
	}

	fn user_texts(&self) -> Vec<String> {
		self.rows()
			.into_iter()
			.filter(|row| row.role == Role::User)
			.flat_map(|row| row.blocks)
			.filter_map(|block| match block {
				Block::Text(text) => Some(text),
				_ => None,
			})
			.collect()
	}

	fn assistant_text(&self) -> Option<String> {
		let text: String = self
			.rows()
			.into_iter()
			.filter(|row| row.role == Role::Assistant && !row.streaming)
			.flat_map(|row| row.blocks)
			.filter_map(|block| match block {
				Block::Text(text) => Some(text),
				_ => None,
			})
			.collect();
		(!text.is_empty()).then_some(text)
	}

	fn tool_runs_running(&self, name: &str) -> bool {
		self.tool_runs()
			.iter()
			.any(|(_, run)| run.name == name && run.status == omp_store::tools::Status::Running)
	}

	fn last_edit_diff(&self) -> Option<omp_store::tools::Diff> {
		self.tool_runs().iter().filter(|(_, run)| run.name == "edit").find_map(|(_, run)| run.diff())
	}
}
