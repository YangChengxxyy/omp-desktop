//! Headless UI test: the real `ChatView` against a real `omp --mode rpc-ui`.
//! It calls a model, so it is ignored by default:
//!
//! cargo test -p omp-desktop -- --ignored --nocapture

use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Bounds, Point, TestAppContext, WindowBounds, WindowOptions, px, size};
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
	let approve = options
		.iter()
		.position(|label| label.starts_with("Approve") || label.contains("允许") || label.contains("批准"))
		.unwrap_or_else(|| panic!("no approve option in {options:?}"));
	cx.update_window(handle, |_, window, cx| window.click(("select-option", approve), cx)).unwrap();

	wait_until(cx, Duration::from_secs(120), "turn settles", |cx| view.read_with(cx, |v, _| v.is_idle()));
	assert_eq!(std::fs::read_to_string(&target).ok().as_deref().map(str::trim), Some("ok"));
	assert!(view.read_with(cx, |v, cx| v.open_dialog_options(cx).is_none()), "dialog closes after answering");
	_ = std::fs::remove_dir_all(&dir);
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
}
