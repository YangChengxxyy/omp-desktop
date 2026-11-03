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
