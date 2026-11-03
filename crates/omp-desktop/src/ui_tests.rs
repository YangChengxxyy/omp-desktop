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

use std::path::Path;

use crate::chat::ChatView;
use crate::session::Notice;
use crate::workspace::Workspace;

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

#[gpui_kit::test]
#[ignore = "spawns omp and calls a model"]
async fn cmd_enter_queues_a_steer(cx: &mut TestAppContext) {
	let (handle, view, dir) = open_chat(cx, "steer", &["--tools", "bash", "--approval-mode", "yolo"]);
	type_and(cx, handle, "Run this exact bash command and nothing else: sleep 60", "enter");
	wait_bash_running(cx, &view);

	// ⌘Enter while busy queues a steer, which omp injects at the next tool
	// boundary (it backgrounds the running bash to do so).
	type_and(cx, handle, "STEER-B: stop and reply only OK", "secondary-enter");
	wait_until(cx, Duration::from_secs(20), "steer queued", |cx| {
		view.read_with(cx, |v, _| v.queue_log().iter().any(|q| q.steering == ["STEER-B: stop and reply only OK"]))
	});
	assert!(view.read_with(cx, |v, _| v.queue_log().iter().all(|q| q.follow_up.is_empty())), "⌘Enter queued a follow-up");
	wait_until(cx, Duration::from_secs(120), "steer delivered", |cx| {
		view.read_with(cx, |v, _| v.user_texts().iter().any(|t| t.starts_with("STEER-B")))
	});
	cx.update_window(handle, |_, window, cx| window.press("escape", cx)).unwrap();
	wait_until(cx, Duration::from_secs(120), "run settles", |cx| view.read_with(cx, |v, _| v.is_idle()));
	_ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
#[ignore = "spawns omp and calls a model"]
async fn chip_promotes_follow_up_to_steer(cx: &mut TestAppContext) {
	let (handle, view, dir) = open_chat(cx, "promote", &["--tools", "bash", "--approval-mode", "yolo"]);
	type_and(cx, handle, "Run this exact bash command and nothing else: sleep 60", "enter");
	wait_bash_running(cx, &view);

	// Enter queues a follow-up; its chip's 插队 button promotes it to a steer.
	type_and(cx, handle, "FOLLOW-A: reply only OK", "enter");
	wait_until(cx, Duration::from_secs(20), "follow-up chip", |cx| {
		view.read_with(cx, |v, _| v.queue().follow_up == ["FOLLOW-A: reply only OK"])
	});
	cx.update_window(handle, |_, window, cx| window.click(("queue-promote", 0usize), cx)).unwrap();
	wait_until(cx, Duration::from_secs(20), "promoted to steer", |cx| {
		view.read_with(cx, |v, _| {
			v.queue_log().iter().any(|q| q.steering == ["FOLLOW-A: reply only OK"] && q.follow_up.is_empty())
		})
	});
	// A follow-up would wait for `sleep 60`; the steer arrives long before.
	wait_until(cx, Duration::from_secs(45), "promoted message delivered", |cx| {
		view.read_with(cx, |v, _| v.user_texts().iter().any(|t| t.starts_with("FOLLOW-A")))
	});
	cx.update_window(handle, |_, window, cx| window.press("escape", cx)).unwrap();
	wait_until(cx, Duration::from_secs(120), "run settles", |cx| view.read_with(cx, |v, _| v.is_idle()));
	_ = std::fs::remove_dir_all(&dir);
}

fn wait_bash_running(cx: &mut TestAppContext, view: &gpui_kit::Entity<ChatView>) {
	wait_until(cx, Duration::from_secs(120), "bash starts", |cx| view.read_with(cx, |v, _| v.tool_runs_running("bash")));
}

/// No model call: a fixture extension command opens the dialogs.
#[gpui_kit::test]
#[ignore = "spawns omp"]
async fn confirm_input_and_editor_dialogs_round_trip(cx: &mut TestAppContext) {
	let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/dialogs.ts");
	let (handle, view, dir) = open_chat(cx, "dialogs", &["--no-extensions", "-e", fixture]);
	type_and(cx, handle, "/dialogs", "enter");
	let wait_kind = |cx: &mut TestAppContext, kind: &'static str| {
		wait_until(cx, Duration::from_secs(30), kind, |cx| view.read_with(cx, |v, cx| v.open_dialog_kind(cx) == Some(kind)))
	};

	wait_kind(cx, "confirm");
	cx.update_window(handle, |_, window, cx| window.click("dialog-yes", cx)).unwrap();
	wait_kind(cx, "input");
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.input("Alice", cx);
		window.click("dialog-submit", cx);
	})
	.unwrap();
	wait_kind(cx, "editor");
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.input("改", cx);
		window.click("dialog-submit", cx);
	})
	.unwrap();

	wait_until(cx, Duration::from_secs(30), "result notice", |cx| {
		view.read_with(cx, |v, _| v.notices().iter().any(|n| matches!(n, Notice::Info(t) if t.starts_with("dialogs "))))
	});
	let notice = view.read_with(cx, |v, _| {
		v.notices().iter().find_map(|n| match n {
			Notice::Info(t) if t.starts_with("dialogs ") => Some(t.clone()),
			_ => None,
		})
	});
	let notice = notice.unwrap();
	assert!(notice.contains("confirmed=true"), "{notice}");
	assert!(notice.contains("name=Alice"), "{notice}");
	let text = notice.split("text=").nth(1).unwrap_or_default();
	assert!(text.contains("预填") && text.contains("改"), "editor keeps the prefill and adds input: {notice}");
	assert!(view.read_with(cx, |v, cx| v.open_dialog_kind(cx).is_none()));
	_ = std::fs::remove_dir_all(&dir);
}

/// M3 acceptance: a session created outside the app (by `omp -p`, which
/// writes the same session files as the TUI) shows up in the sidebar and can
/// be continued. Also covers reconnecting a killed process with `--resume`,
/// fork, switching to an already open session, and closing a tab.
#[gpui_kit::test]
#[ignore = "spawns omp and calls a model"]
async fn external_session_is_listed_resumed_reconnected_and_forked(cx: &mut TestAppContext) {
	let root = std::env::temp_dir().join(format!("omp-desktop-m3-{}", std::process::id()));
	_ = std::fs::remove_dir_all(&root);
	let (sessions, work) = (root.join("sessions"), root.join("work"));
	std::fs::create_dir_all(&sessions).unwrap();
	std::fs::create_dir_all(&work).unwrap();
	let status = std::process::Command::new("omp")
		.args(["-p", "--no-tools", "--thinking", "off", "--session-dir"])
		.arg(&sessions)
		.arg("--cwd")
		.arg(&work)
		.arg("记住暗号：蓝鲸42。只回复：好")
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.status()
		.expect("run omp -p");
	assert!(status.success());

	let (handle, workspace) = open_workspace(cx, &sessions, &work);
	wait_until(cx, Duration::from_secs(30), "session listed", |cx| workspace.read_with(cx, |w, _| w.listed().len() == 1));
	let original = workspace.read_with(cx, |w, _| w.listed()[0].path.clone());

	// Open it: a second tab resumes the file and shows its history.
	cx.update_window(handle, |_, window, cx| window.click(("session-row", 0usize), cx)).unwrap();
	assert_eq!(workspace.read_with(cx, |w, _| (w.tab_views().len(), w.active_index())), (2, 1));
	let tab = workspace.read_with(cx, |w, _| w.tab_views()[1].clone());
	wait_until(cx, Duration::from_secs(60), "resumed tab connects", |cx| tab.read_with(cx, |v, _| v.is_connected()));
	assert_eq!(tab.read_with(cx, |v, _| v.user_texts()), ["记住暗号：蓝鲸42。只回复：好"]);
	assert_eq!(tab.read_with(cx, |v, _| v.session_file().map(Path::to_owned)), Some(original.clone()));

	// Clicking the same session again switches to its tab.
	cx.update_window(handle, |_, window, cx| {
		window.press("secondary-1", cx);
		window.click(("session-row", 0usize), cx);
	})
	.unwrap();
	assert_eq!(workspace.read_with(cx, |w, _| (w.tab_views().len(), w.active_index())), (2, 1));

	// Continue the conversation; the reply depends on the resumed history.
	type_and(cx, handle, "暗号是什么？只回复暗号本身", "enter");
	wait_until(cx, Duration::from_secs(120), "continued reply", |cx| {
		tab.read_with(cx, |v, _| v.is_idle() && v.assistant_texts().len() == 2)
	});
	assert!(tab.read_with(cx, |v, _| v.assistant_texts()[1].contains("蓝鲸42")), "{:?}", tab.read_with(cx, |v, _| v.assistant_texts()));
	assert!(std::fs::read_to_string(&original).unwrap().contains("暗号是什么"), "turn persisted to the session file");

	// Kill the process: the tab shows the banner and reconnects with --resume.
	let pid = tab.read_with(cx, |v, _| v.pid()).expect("pid");
	std::process::Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap();
	wait_until(cx, Duration::from_secs(30), "disconnect noticed", |cx| tab.read_with(cx, |v, _| v.is_disconnected()));
	cx.update_window(handle, |_, window, cx| window.click("reconnect-banner", cx)).unwrap();
	wait_until(cx, Duration::from_secs(60), "reconnected", |cx| tab.read_with(cx, |v, _| v.is_connected()));
	assert_ne!(tab.read_with(cx, |v, _| v.pid()), Some(pid));
	assert_eq!(tab.read_with(cx, |v, _| v.user_texts()).len(), 2, "history reloaded from the file");

	// Fork: a third tab continues a copy in a new file.
	cx.update_window(handle, |_, window, cx| window.click(("session-fork", 0usize), cx)).unwrap();
	let fork = workspace.read_with(cx, |w, _| w.tab_views()[2].clone());
	wait_until(cx, Duration::from_secs(60), "fork connects", |cx| {
		fork.read_with(cx, |v, _| v.is_connected() && v.session_file().is_some_and(|f| f != original))
	});
	assert_eq!(fork.read_with(cx, |v, _| v.user_texts()).len(), 2, "fork carries the history");
	wait_until(cx, Duration::from_secs(30), "fork listed", |cx| workspace.read_with(cx, |w, _| w.listed().len() == 2));

	// Close the forked tab: its process exits.
	let fork_pid = fork.read_with(cx, |v, _| v.pid()).expect("fork pid");
	cx.update_window(handle, |_, window, cx| window.press("secondary-w", cx)).unwrap();
	assert_eq!(workspace.read_with(cx, |w, _| w.tab_views().len()), 2);
	wait_until(cx, Duration::from_secs(10), "forked process exits", |_| !process_alive(fork_pid));
	_ = std::fs::remove_dir_all(&root);
}

/// ⌘N/⌘1–9/⌘W manage tabs without calling a model.
#[gpui_kit::test]
#[ignore = "spawns omp"]
async fn shortcuts_open_switch_and_close_tabs(cx: &mut TestAppContext) {
	let root = std::env::temp_dir().join(format!("omp-desktop-tabs-{}", std::process::id()));
	_ = std::fs::remove_dir_all(&root);
	std::fs::create_dir_all(root.join("sessions")).unwrap();
	let (handle, workspace) = open_workspace(cx, &root.join("sessions"), &root);
	let press = |cx: &mut TestAppContext, key: &str| cx.update_window(handle, |_, window, cx| window.press(key, cx)).unwrap();
	let tabs = |cx: &mut TestAppContext| workspace.read_with(cx, |w, _| (w.tab_views().len(), w.active_index()));

	press(cx, "secondary-n");
	press(cx, "secondary-n");
	assert_eq!(tabs(cx), (3, 2));
	press(cx, "secondary-1");
	assert_eq!(tabs(cx), (3, 0));
	let views = workspace.read_with(cx, |w, _| w.tab_views());
	for view in &views {
		wait_until(cx, Duration::from_secs(60), "tab connects", |cx| view.read_with(cx, |v, _| v.is_connected()));
	}
	let first_pid = views[0].read_with(cx, |v, _| v.pid()).unwrap();
	press(cx, "secondary-w");
	assert_eq!(tabs(cx), (2, 0));
	wait_until(cx, Duration::from_secs(10), "closed tab's process exits", |_| !process_alive(first_pid));
	press(cx, "secondary-w");
	press(cx, "secondary-w");
	// Closing the last tab leaves a fresh one.
	assert_eq!(tabs(cx).0, 1);
	let fresh = workspace.read_with(cx, |w, _| w.tab_views()[0].clone());
	wait_until(cx, Duration::from_secs(60), "fresh tab connects", |cx| fresh.read_with(cx, |v, _| v.is_connected()));
	// A new session writes no file until its first message.
	assert!(workspace.read_with(cx, |w, _| w.listed().is_empty()));
	_ = std::fs::remove_dir_all(&root);
}

fn open_workspace(
	cx: &mut TestAppContext,
	sessions: &Path,
	cwd: &Path,
) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<Workspace>) {
	cx.executor().allow_parking();
	cx.update(gpui_kit::init);
	cx.update(crate::theme::apply);
	let base = SessionOptions {
		session_dir: Some(sessions.to_owned()),
		extra_args: ["--thinking", "off", "--no-tools"].map(String::from).to_vec(),
		..Default::default()
	};
	let cwd = cwd.to_owned();
	cx.update(|cx| {
		gpui_kit::open_window(
			WindowOptions {
				window_bounds: Some(WindowBounds::Windowed(Bounds { origin: Point::default(), size: size(px(1200.), px(800.)) })),
				..Default::default()
			},
			cx,
			|window, cx| cx.new(|cx| Workspace::new(base, cwd, window, cx)),
		)
		.expect("open window")
	})
}

fn process_alive(pid: u32) -> bool {
	std::process::Command::new("kill")
		.args(["-0", &pid.to_string()])
		.stderr(std::process::Stdio::null())
		.status()
		.is_ok_and(|s| s.success())
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

	/// Text of each finished assistant message that has text.
	fn assistant_texts(&self) -> Vec<String> {
		self.rows()
			.into_iter()
			.filter(|row| row.role == Role::Assistant && !row.streaming)
			.map(|row| {
				row.blocks
					.into_iter()
					.filter_map(|block| match block {
						Block::Text(text) => Some(text),
						_ => None,
					})
					.collect::<String>()
			})
			.filter(|text| !text.is_empty())
			.collect()
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

/// M4 acceptance without a model call: every `get_state` field shows in the
/// detail panel, and the status-bar menus, auto-compact switch and compact
/// button drive the session. Model and thinking changes are session-only.
#[gpui_kit::test]
#[ignore = "spawns omp"]
async fn status_bar_controls_and_detail_panel(cx: &mut TestAppContext) {
	let (handle, view, dir) = open_chat(cx, "status", &["--no-tools"]);
	wait_until(cx, Duration::from_secs(30), "state, stats and choices load", |cx| {
		view.read_with(cx, |v, _| v.state().is_some() && v.has_stats() && v.choices().0 > 1 && v.choices().1 > 1)
	});

	// The panel lists every field `get_state` sent, plus the optional ones.
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.click("status-panel", cx);
		window.render_frame(cx);
	})
	.unwrap();
	let state = view.read_with(cx, |v, _| v.state().cloned().unwrap());
	let mut keys: Vec<String> = serde_json::to_value(&state).unwrap().as_object().unwrap().keys().cloned().collect();
	keys.extend(["model", "goal", "sessionName", "sessionFile"].map(String::from));
	cx.update_window(handle, |_, window, _| {
		for key in &keys {
			assert!(window.try_find(format!("state-{key}")).is_some(), "panel has no row for {key}");
		}
	})
	.unwrap();

	// Thinking menu: pick the first level that is not the current one.
	let (levels, current) = view.read_with(cx, |v, _| (v.thinking_levels_for_test(), v.state().unwrap().thinking_level));
	let (pick, level) = levels.iter().copied().enumerate().find(|(_, l)| Some(*l) != current).expect("another level");
	cx.update_window(handle, |_, window, cx| {
		window.click("status-thinking", cx);
		window.render_frame(cx);
		window.within("popup-menu").click(pick, cx);
	})
	.unwrap();
	wait_until(cx, Duration::from_secs(20), "thinking level applied", |cx| {
		view.read_with(cx, |v, _| !v.control_pending() && v.state().unwrap().thinking_level == Some(level))
	});

	// Model menu: pick another model.
	let (models, current) = view.read_with(cx, |v, _| {
		(v.models_for_test(), v.state().unwrap().model.as_ref().map(|m| (m.provider.clone(), m.id.clone())))
	});
	let (pick, target) = models.iter().cloned().enumerate().find(|(_, m)| Some(m) != current.as_ref()).expect("another model");
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.click("status-model", cx);
		window.render_frame(cx);
		window.within("popup-menu").click(pick, cx);
	})
	.unwrap();
	wait_until(cx, Duration::from_secs(20), "model applied", |cx| {
		view.read_with(cx, |v, _| {
			!v.control_pending() && v.state().unwrap().model.as_ref().map(|m| (m.provider.clone(), m.id.clone())) == Some(target.clone())
		})
	});

	// Auto-compact switch.
	let auto = view.read_with(cx, |v, _| v.state().unwrap().auto_compaction_enabled);
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.click("panel-auto-compact", cx);
	})
	.unwrap();
	wait_until(cx, Duration::from_secs(20), "auto-compact toggled", |cx| {
		view.read_with(cx, |v, _| !v.control_pending() && v.state().unwrap().auto_compaction_enabled == !auto)
	});

	// Compact on an empty session: omp skips it and says why.
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.click("status-compact", cx);
	})
	.unwrap();
	wait_until(cx, Duration::from_secs(30), "compact reports skip", |cx| {
		view.read_with(cx, |v, _| {
			!v.control_pending() && v.notices().iter().any(|n| matches!(n, Notice::Info(t) if t.starts_with("压缩未执行")))
		})
	});
	assert!(
		view.read_with(cx, |v, _| v.notices().iter().all(|n| !matches!(n, Notice::Error(_)))),
		"unexpected error notice: {:?}",
		view.read_with(cx, |v, _| v.notices().to_vec())
	);
	cx.update_window(handle, |_, window, cx| window.render_frame(cx)).unwrap();
	_ = std::fs::remove_dir_all(&dir);
}

/// A real subagent shows in the panel with live progress; 干预 sends a steer
/// and 取消 kills it.
#[gpui_kit::test]
#[ignore = "spawns omp and calls a model"]
async fn subagent_panel_steers_and_cancels(cx: &mut TestAppContext) {
	let (handle, view, dir) = open_chat(cx, "subagent", &["--tools", "task,bash", "--approval-mode", "yolo"]);
	type_and(
		cx,
		handle,
		"Call the task tool exactly once with a single task (agent: task) whose assignment is: \
		 run the bash command `sleep 120`, then reply DONE. Do nothing else yourself.",
		"enter",
	);
	wait_until(cx, Duration::from_secs(180), "subagent running", |cx| {
		view.read_with(cx, |v, _| v.subagents().iter().any(|s| s.running() && s.id.is_some()))
	});
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.click("status-subagents", cx);
		window.render_frame(cx);
		window.click(("subagent-steer", 0usize), cx);
		window.render_frame(cx);
		window.click(("subagent-steer-input", 0usize), cx);
		window.input("keep going", cx);
		window.press("enter", cx);
		window.render_frame(cx);
	})
	.unwrap();
	wait_until(cx, Duration::from_secs(5), "steer input closes", |cx| {
		cx.update_window(handle, |_, window, _| window.try_find(("subagent-steer-input", 0usize)).is_none()).unwrap()
	});
	cx.update_window(handle, |_, window, cx| window.click(("subagent-cancel", 0usize), cx)).unwrap();
	wait_until(cx, Duration::from_secs(60), "subagent aborted", |cx| {
		view.read_with(cx, |v, _| v.subagents().first().is_some_and(|s| s.status == omp_rpc::SubagentStatus::Aborted))
	});
	let errors: Vec<_> =
		view.read_with(cx, |v, _| v.notices().iter().filter(|n| matches!(n, Notice::Error(_))).cloned().collect());
	assert!(errors.is_empty(), "error notices: {errors:?}");
	cx.update_window(handle, |_, window, cx| window.press("escape", cx)).unwrap();
	wait_until(cx, Duration::from_secs(120), "run settles", |cx| view.read_with(cx, |v, _| v.is_idle()));
	_ = std::fs::remove_dir_all(&dir);
}

/// A solid-colour PNG, built by hand so the test needs no image crate.
fn solid_png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
	fn crc32(bytes: &[u8]) -> u32 {
		let mut crc = !0u32;
		for &b in bytes {
			crc ^= b as u32;
			for _ in 0..8 {
				crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
			}
		}
		!crc
	}
	fn adler32(bytes: &[u8]) -> u32 {
		let (mut a, mut b) = (1u32, 0u32);
		for &x in bytes {
			a = (a + x as u32) % 65521;
			b = (b + a) % 65521;
		}
		b << 16 | a
	}
	fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
		out.extend((data.len() as u32).to_be_bytes());
		let start = out.len();
		out.extend(kind);
		out.extend(data);
		let crc = crc32(&out[start..]);
		out.extend(crc.to_be_bytes());
	}
	let mut raw = Vec::new();
	for _ in 0..height {
		raw.push(0);
		for _ in 0..width {
			raw.extend(rgb);
		}
	}
	// zlib with stored (uncompressed) deflate blocks.
	let mut zlib = vec![0x78, 0x01];
	let blocks: Vec<_> = raw.chunks(65_535).collect();
	for (i, block) in blocks.iter().enumerate() {
		zlib.push(u8::from(i + 1 == blocks.len()));
		let len = block.len() as u16;
		zlib.extend(len.to_le_bytes());
		zlib.extend((!len).to_le_bytes());
		zlib.extend(*block);
	}
	zlib.extend(adler32(&raw).to_be_bytes());
	let mut ihdr = Vec::new();
	ihdr.extend(width.to_be_bytes());
	ihdr.extend(height.to_be_bytes());
	ihdr.extend([8, 2, 0, 0, 0]);
	let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
	chunk(&mut png, b"IHDR", &ihdr);
	chunk(&mut png, b"IDAT", &zlib);
	chunk(&mut png, b"IEND", &[]);
	png
}

/// ⌘V with an image on the clipboard attaches it (text paste still types),
/// × removes it, and the sent prompt carries the image to the model, which
/// can see it. The transcript then shows the image, not a placeholder.
#[gpui_kit::test]
#[ignore = "spawns omp and calls a model"]
async fn pasted_image_reaches_the_model(cx: &mut TestAppContext) {
	use gpui_kit::{ClipboardItem, Image, ImageFormat};
	let (handle, view, dir) = open_chat(cx, "image", &["--no-tools"]);
	let paste = |cx: &mut TestAppContext, item: ClipboardItem| {
		cx.write_to_clipboard(item);
		cx.update_window(handle, |_, window, cx| {
			window.render_frame(cx);
			window.click("composer", cx);
			window.press("cmd-v", cx);
			window.render_frame(cx);
		})
		.unwrap();
	};

	paste(cx, ClipboardItem::new_string("颜色？".into()));
	assert_eq!(view.read_with(cx, |v, cx| v.composer_text(cx)), "颜色？", "text paste still types");

	let red = Image::from_bytes(ImageFormat::Png, solid_png(64, 64, [255, 0, 0]));
	paste(cx, ClipboardItem::new_image(&red));
	paste(cx, ClipboardItem::new_image(&red));
	assert_eq!(view.read_with(cx, |v, _| v.attachment_labels()), ["图片.png"], "same image attaches once");
	assert_eq!(view.read_with(cx, |v, cx| v.composer_text(cx)), "颜色？", "image paste adds no text");

	let blue = Image::from_bytes(ImageFormat::Png, solid_png(64, 64, [0, 0, 255]));
	paste(cx, ClipboardItem::new_image(&blue));
	assert_eq!(view.read_with(cx, |v, _| v.attachment_labels().len()), 2);
	cx.update_window(handle, |_, window, cx| window.click(("attachment-remove", 1usize), cx)).unwrap();
	assert_eq!(view.read_with(cx, |v, _| v.attachment_labels().len()), 1, "× removes the blue one");

	cx.update_window(handle, |_, window, cx| {
		window.input("只用一个中文词回答这张图片的颜色", cx);
		window.press("enter", cx);
	})
	.unwrap();
	assert!(view.read_with(cx, |v, _| v.attachment_labels().is_empty()), "attachments clear on send");
	wait_until(cx, Duration::from_secs(120), "reply settles", |cx| {
		view.read_with(cx, |v, _| v.is_idle() && v.assistant_text().is_some())
	});
	let reply = view.read_with(cx, |v, _| v.assistant_text()).unwrap();
	assert!(reply.contains('红'), "model did not see the red image: {reply:?}");
	let (row, user) = view.read_with(cx, |v, _| {
		v.rows().into_iter().enumerate().find(|(_, r)| r.role == Role::User).map(|(i, r)| (i, r.blocks)).unwrap()
	});
	let block = user
		.iter()
		.position(|b| matches!(b, Block::Image { data, .. } if !data.is_empty()))
		.unwrap_or_else(|| panic!("user message has no image: {user:?}"));
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		assert!(window.try_find(("transcript-image", row * 64 + block)).is_some(), "transcript shows no image");
	})
	.unwrap();
	_ = std::fs::remove_dir_all(&dir);
}

/// Image files dropped on the tab attach; other files are reported, not sent.
#[gpui_kit::test]
#[ignore = "spawns omp"]
async fn dropped_image_files_attach(cx: &mut TestAppContext) {
	use gpui_kit::{ExternalPaths, FileDropEvent, PlatformInput};
	let (handle, view, dir) = open_chat(cx, "drop", &["--no-tools"]);
	let png = dir.join("红色.png");
	std::fs::write(&png, solid_png(8, 8, [255, 0, 0])).unwrap();
	let txt = dir.join("notes.txt");
	std::fs::write(&txt, "hi").unwrap();
	let position = Point::new(px(400.), px(300.));
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		let paths = ExternalPaths(vec![png.clone(), txt.clone()].into());
		for event in [
			FileDropEvent::Entered { position, paths },
			FileDropEvent::Pending { position },
			FileDropEvent::Submit { position },
		] {
			window.dispatch_event(PlatformInput::FileDrop(event), cx);
		}
		window.render_frame(cx);
	})
	.unwrap();
	assert_eq!(view.read_with(cx, |v, _| v.attachment_labels()), ["红色.png"]);
	assert!(
		view.read_with(cx, |v, _| v.notices().iter().any(|n| matches!(n, Notice::Info(t) if t.contains("notes.txt")))),
		"non-image file not reported"
	);
	cx.update_window(handle, |_, window, _| assert!(window.try_find(("attachment", 0usize)).is_some())).unwrap();
	_ = std::fs::remove_dir_all(&dir);
}

/// No model call: `/` opens completions from omp's catalog; typing narrows,
/// ↓ moves, Tab completes a command and then its subcommand, Esc hides the
/// popup, and Enter on the full command runs it in omp, whose output shows
/// in the transcript without the composer staying busy.
#[gpui_kit::test]
#[ignore = "spawns omp"]
async fn slash_commands_complete_and_run(cx: &mut TestAppContext) {
	let (handle, view, dir) = open_chat(cx, "slash", &["--no-tools"]);
	wait_until(cx, Duration::from_secs(30), "command catalog", |cx| view.read_with(cx, |v, _| v.command_count() > 10));
	let popup = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.completion_labels());
	let key = |cx: &mut TestAppContext, key: &str| {
		cx.update_window(handle, |_, window, cx| {
			window.press(key, cx);
			window.render_frame(cx);
		})
		.unwrap();
	};

	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.click("composer", cx);
		window.input("/", cx);
	})
	.unwrap();
	cx.run_until_parked();
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		assert!(window.try_find("slash-popup").is_some(), "popup not shown");
	})
	.unwrap();
	let (all, _) = popup(cx).expect("popup");
	assert!(all.len() > 10 && all.iter().any(|l| l == "/fast"), "{all:?}");

	cx.update_window(handle, |_, window, cx| window.input("fa", cx)).unwrap();
	let (rows, selected) = popup(cx).expect("narrowed");
	assert_eq!((rows[0].as_str(), selected), ("/fast", 0), "{rows:?}");

	// Tab completes the command, then offers its subcommands.
	key(cx, "tab");
	assert_eq!(view.read_with(cx, |v, cx| v.composer_text(cx)), "/fast ");
	let (subs, _) = popup(cx).expect("subcommands");
	assert!(subs.contains(&"/fast status".to_owned()), "{subs:?}");
	let status = subs.iter().position(|s| s == "/fast status").unwrap();
	for _ in 0..status {
		key(cx, "down");
	}
	assert_eq!(popup(cx).unwrap().1, status);
	key(cx, "enter");
	assert_eq!(view.read_with(cx, |v, cx| v.composer_text(cx)), "/fast status ", "Enter completes a partial line");
	assert!(popup(cx).is_none(), "exact subcommand closes the popup");

	// Esc hides the popup without clearing the text or aborting.
	cx.update_window(handle, |_, window, cx| {
		window.press("backspace", cx);
		window.press("backspace", cx);
	})
	.unwrap();
	assert!(popup(cx).is_some(), "editing reopens it");
	key(cx, "escape");
	assert!(popup(cx).is_none());
	assert_eq!(view.read_with(cx, |v, cx| v.composer_text(cx)), "/fast statu");
	assert!(view.read_with(cx, |v, _| v.is_idle()), "Esc on the popup must not abort");

	cx.update_window(handle, |_, window, cx| {
		window.input("s", cx);
		window.press("enter", cx);
	})
	.unwrap();
	wait_until(cx, Duration::from_secs(20), "command output", |cx| {
		view.read_with(cx, |v, _| {
			v.rows().iter().any(|r| r.role == Role::Command && matches!(&r.blocks[..], [Block::Output(t)] if t.contains("Fast mode")))
		})
	});
	wait_until(cx, Duration::from_secs(10), "composer idle", |cx| view.read_with(cx, |v, _| v.is_idle()));
	assert_eq!(view.read_with(cx, |v, cx| v.composer_text(cx)), "");
	assert!(view.read_with(cx, |v, _| v.assistant_text().is_none()), "a builtin command must not call the model");
	cx.update_window(handle, |_, window, cx| window.render_frame(cx)).unwrap();
	_ = std::fs::remove_dir_all(&dir);
}

/// A scratch `PI_CODING_AGENT_DIR` holding the given fake API-key
/// credentials, so login tests never touch `~/.omp`.
fn scratch_agent_dir(name: &str, providers: &[&str]) -> std::path::PathBuf {
	let dir = std::env::temp_dir().join(format!("omp-desktop-agent-{name}-{}", std::process::id()));
	_ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).unwrap();
	if !providers.is_empty() {
		let credentials: Vec<_> = providers
			.iter()
			.map(|p| serde_json::json!({"provider": p, "credential": {"type": "api_key", "key": "fake-omp-desktop-test"}}))
			.collect();
		let file = dir.join("import.json");
		std::fs::write(&file, serde_json::json!({"version": 1, "credentials": credentials}).to_string()).unwrap();
		let status = std::process::Command::new("omp")
			.args(["auth", "import"])
			.arg(&file)
			.env("PI_CODING_AGENT_DIR", &dir)
			.stdout(std::process::Stdio::null())
			.status()
			.unwrap();
		assert!(status.success(), "omp auth import");
	}
	dir
}

/// Model-free: with no credentials omp quits at startup; the tab says so and
/// offers the accounts panel instead of a generic failure.
#[gpui_kit::test]
#[ignore = "spawns omp"]
async fn missing_credentials_point_to_login(cx: &mut TestAppContext) {
	cx.executor().allow_parking();
	cx.update(gpui_kit::init);
	cx.update(crate::theme::apply);
	let agent = scratch_agent_dir("nologin", &[]);
	let sessions = std::env::temp_dir().join(format!("omp-desktop-nologin-sessions-{}", std::process::id()));
	_ = std::fs::remove_dir_all(&sessions);
	std::fs::create_dir_all(&sessions).unwrap();
	let mut env = vec![("PI_CODING_AGENT_DIR".to_owned(), agent.display().to_string())];
	// Provider keys in the test environment would let omp start.
	for (key, _) in std::env::vars() {
		if key.ends_with("_API_KEY") || key.ends_with("_OAUTH_TOKEN") {
			env.push((key, String::new()));
		}
	}
	let base = SessionOptions {
		session_dir: Some(sessions.clone()),
		extra_args: ["--thinking", "off", "--no-tools"].map(String::from).to_vec(),
		env,
		..Default::default()
	};
	let cwd = sessions.clone();
	let (handle, workspace) = cx.update(|cx| {
		gpui_kit::open_window(
			WindowOptions {
				window_bounds: Some(WindowBounds::Windowed(Bounds { origin: Point::default(), size: size(px(1200.), px(800.)) })),
				..Default::default()
			},
			cx,
			|window, cx| cx.new(|cx| Workspace::new(base, cwd, window, cx)),
		)
		.expect("open window")
	});
	wait_until(cx, Duration::from_secs(60), "needs-login screen", |cx| {
		cx.update_window(handle, |_, window, cx| {
			window.render_frame(cx);
			window.try_find("needs-login").is_some()
		})
		.unwrap()
	});
	cx.update_window(handle, |_, window, cx| {
		window.click("needs-login-open", cx);
		window.render_frame(cx);
	})
	.unwrap();
	let accounts = workspace.read_with(cx, |w, _| w.accounts_view()).expect("accounts panel opened");
	// The panel's own process starts despite the missing credentials.
	wait_until(cx, Duration::from_secs(60), "accounts connect", |cx| accounts.read_with(cx, |v, _| v.connected()));
	assert!(accounts.read_with(cx, |v, _| v.providers().len() > 10));
	assert!(accounts.read_with(cx, |v, _| v.providers().iter().all(|p| !p.authenticated)), "placeholder key is not stored");
	cx.update_window(handle, |_, window, cx| {
		window.press("escape", cx);
	})
	.unwrap();
	assert!(workspace.read_with(cx, |w, _| w.accounts_view()).is_none());
	_ = std::fs::remove_dir_all(&agent);
	_ = std::fs::remove_dir_all(&sessions);
}

/// Model-free, in a scratch agent dir: the accounts panel lists providers
/// with the stored one marked, search narrows the list, a login shows the
/// authorize link (opened in the browser, copyable) and the code prompt,
/// cancelling restarts cleanly, and logout removes the credential after a
/// confirmation.
#[gpui_kit::test]
#[ignore = "spawns omp"]
async fn accounts_login_flow_and_logout(cx: &mut TestAppContext) {
	cx.executor().allow_parking();
	cx.update(gpui_kit::init);
	cx.update(crate::theme::apply);
	let agent = scratch_agent_dir("accounts", &["openrouter"]);
	let base = SessionOptions { env: vec![("PI_CODING_AGENT_DIR".to_owned(), agent.display().to_string())], ..Default::default() };
	let (handle, accounts) = cx.update(|cx| {
		gpui_kit::open_window(
			WindowOptions {
				window_bounds: Some(WindowBounds::Windowed(Bounds { origin: Point::default(), size: size(px(900.), px(800.)) })),
				..Default::default()
			},
			cx,
			|window, cx| cx.new(|cx| crate::accounts::AccountsView::new(&base, window, cx)),
		)
		.expect("open window")
	});
	wait_until(cx, Duration::from_secs(60), "accounts connect", |cx| accounts.read_with(cx, |v, _| v.connected()));
	let authenticated = |cx: &mut TestAppContext| {
		accounts.read_with(cx, |v, _| v.providers().iter().filter(|p| p.authenticated).map(|p| p.id.clone()).collect::<Vec<_>>())
	};
	assert_eq!(authenticated(cx), ["openrouter"]);

	// The stored provider sorts first; search narrows to codex.
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		assert!(window.try_find(("provider-accounts", 0usize)).is_some(), "authenticated row offers its accounts");
		window.click("accounts-search", cx);
		window.input("codex device", cx);
		window.render_frame(cx);
		// openai-codex-device only.
		assert!(window.try_find(("provider", 1usize)).is_none(), "search narrows to one row");
		window.click(("provider-login", 0usize), cx);
	})
	.unwrap();
	wait_until(cx, Duration::from_secs(30), "device login shows its link", |cx| {
		accounts.read_with(cx, |v, _| v.flow().is_some_and(|f| f.url.is_some()))
	});
	let url = accounts.read_with(cx, |v, _| v.flow().unwrap().url.clone().unwrap());
	let login_pid = accounts.read_with(cx, |v, _| v.login_pid()).expect("login process");
	assert_eq!(cx.opened_url().as_deref(), Some(url.as_str()), "the browser opens the authorize URL");
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.click("login-copy", cx);
	})
	.unwrap();
	let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
	assert_eq!(Some(copied.as_str()), accounts.read_with(cx, |v, _| v.flow().unwrap().copy_url.clone()).as_deref());

	cx.update_window(handle, |_, window, cx| window.click("login-cancel", cx)).unwrap();
	assert!(accounts.read_with(cx, |v, _| v.flow().is_none()));
	wait_until(cx, Duration::from_secs(60), "restarted after cancel", |cx| accounts.read_with(cx, |v, _| v.connected()));
	assert_eq!(authenticated(cx), ["openrouter"], "cancel stored nothing");
	wait_until(cx, Duration::from_secs(10), "cancelled login process exits", |_| !process_alive(login_pid));

	// Logout: list the account, confirm, and it is gone.
	cx.update_window(handle, |_, window, cx| {
		window.click("accounts-search", cx);
		window.press("cmd-a", cx);
		window.press("backspace", cx);
		window.render_frame(cx);
		window.click(("provider-accounts", 0usize), cx);
	})
	.unwrap();
	wait_until(cx, Duration::from_secs(10), "accounts listed", |cx| {
		accounts.read_with(cx, |v, _| v.listed_accounts().is_some_and(|a| a.len() == 1))
	});
	cx.update_window(handle, |_, window, cx| {
		window.render_frame(cx);
		window.click(("account-logout", 0usize), cx);
		window.render_frame(cx);
		window.click(("account-confirm", 0usize), cx);
	})
	.unwrap();
	wait_until(cx, Duration::from_secs(10), "logged out", |cx| authenticated(cx).is_empty());
	let message = accounts.read_with(cx, |v, _| v.message().cloned());
	assert!(matches!(&message, Some(crate::accounts::Message::Info(t)) if t.contains("已移除")), "{message:?}");
	let export = std::process::Command::new("omp")
		.args(["auth", "export"])
		.env("PI_CODING_AGENT_DIR", &agent)
		.output()
		.unwrap();
	assert!(!String::from_utf8_lossy(&export.stdout).contains("openrouter"), "credential removed from the store");
	drop(accounts);
	_ = std::fs::remove_dir_all(&agent);
}

struct Blank;

impl gpui_kit::Render for Blank {
	fn render(&mut self, _: &mut gpui_kit::Window, _: &mut gpui_kit::Context<Self>) -> impl gpui_kit::IntoElement {
		gpui_kit::div()
	}
}

/// Model-free and omp-free: a manual theme choice overrides the system
/// appearance; "follow the system" goes back to it (the test window is light).
#[gpui_kit::test]
async fn theme_choice_overrides_and_follows_system(cx: &mut TestAppContext) {
	use gpui_kit::component::ActiveTheme as _;
	use crate::theme::ThemeChoice;
	cx.update(gpui_kit::init);
	cx.update(crate::theme::apply);
	let (handle, _) = cx.update(|cx| gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Blank)).unwrap());
	let dark = |cx: &mut TestAppContext| cx.read(|cx| (cx.theme().is_dark(), crate::theme::palette(cx).background));
	assert_eq!(dark(cx), (false, crate::theme::LIGHT.background));
	for (choice, expect) in [
		(ThemeChoice::Dark, (true, crate::theme::DARK.background)),
		(ThemeChoice::Light, (false, crate::theme::LIGHT.background)),
		(ThemeChoice::Dark, (true, crate::theme::DARK.background)),
		(ThemeChoice::System, (false, crate::theme::LIGHT.background)),
	] {
		cx.update_window(handle, |_, window, cx| crate::theme::set_choice(choice, window, cx)).unwrap();
		assert_eq!(dark(cx), expect, "{choice:?}");
		assert_eq!(cx.read(crate::theme::choice), choice);
	}
	// An appearance change leaves a manual choice alone.
	cx.update_window(handle, |_, window, cx| {
		crate::theme::set_choice(ThemeChoice::Dark, window, cx);
		crate::theme::appearance_changed(window, cx);
	})
	.unwrap();
	assert!(dark(cx).0);
}

/// Model-free: ⌘K searches actions, models, and slash commands and runs the
/// pick; ⌘M narrows to models starting on the current one; ⌘T cycles the
/// thinking level.
#[gpui_kit::test]
#[ignore = "spawns omp"]
async fn palette_and_model_shortcuts(cx: &mut TestAppContext) {
	use gpui_kit::component::ActiveTheme as _;
	let root = std::env::temp_dir().join(format!("omp-desktop-palette-{}", std::process::id()));
	_ = std::fs::remove_dir_all(&root);
	std::fs::create_dir_all(root.join("sessions")).unwrap();
	let (handle, workspace) = open_workspace(cx, &root.join("sessions"), &root);
	let view = workspace.read_with(cx, |w, _| w.tab_views()[0].clone());
	wait_until(cx, Duration::from_secs(60), "choices and commands load", |cx| {
		view.read_with(cx, |v, _| v.is_connected() && v.state().is_some() && v.choices().0 > 1 && v.command_count() > 10)
	});
	let press = |cx: &mut TestAppContext, key: &str| {
		cx.update_window(handle, |_, window, cx| {
			window.press(key, cx);
			window.render_frame(cx);
		})
		.unwrap()
	};
	let typed = |cx: &mut TestAppContext, text: &str| {
		cx.update_window(handle, |_, window, cx| {
			window.input(text, cx);
			window.render_frame(cx);
		})
		.unwrap();
		cx.run_until_parked();
	};
	let palette = |cx: &mut TestAppContext| workspace.read_with(cx, |w, _| w.palette_view());

	// ⌘K → theme action.
	press(cx, "secondary-k");
	let open = palette(cx).expect("⌘K opens the palette");
	typed(cx, "主题 深色");
	assert_eq!(open.read_with(cx, |p, cx| p.titles(cx))[0], "主题：深色");
	press(cx, "enter");
	assert!(palette(cx).is_none(), "running a pick closes the palette");
	assert!(cx.read(|cx| cx.theme().is_dark()));

	// ⌘K → slash command lands in the composer.
	press(cx, "secondary-k");
	typed(cx, "/fast");
	press(cx, "enter");
	assert_eq!(view.read_with(cx, |v, cx| v.composer_text(cx)), "/fast ");

	// ⌘M: models only, starting on the current one; Esc closes.
	press(cx, "secondary-m");
	let models = palette(cx).expect("⌘M opens the model picker");
	let (titles, selected) = models.read_with(cx, |p, cx| (p.titles(cx), p.selected()));
	let current = view.read_with(cx, |v, _| v.state().unwrap().model.clone().unwrap());
	assert_eq!(titles.len(), view.read_with(cx, |v, _| v.choices().0), "models only");
	assert_eq!(titles[selected], current.id, "starts on the current model");
	press(cx, "escape");
	assert!(palette(cx).is_none());
	assert_eq!(view.read_with(cx, |v, cx| v.composer_text(cx)), "/fast ", "Esc in the palette does not touch the composer");

	// ⌘T cycles the thinking level.
	let before = view.read_with(cx, |v, _| v.state().unwrap().thinking_level);
	press(cx, "secondary-t");
	wait_until(cx, Duration::from_secs(20), "thinking level cycles", |cx| {
		view.read_with(cx, |v, _| !v.control_pending() && v.state().unwrap().thinking_level != before)
	});
	_ = std::fs::remove_dir_all(&root);
}
