//! omp-desktop entry point: one window with session tabs, starting with a new
//! session in the launch directory (or `--cwd <dir>`). `--session-dir <dir>`
//! stores and lists sessions there instead of `~/.omp/agent/sessions`.

mod accounts;
mod attachments;
mod chat;
mod copy;
mod blocking;
mod git;
mod packages;
mod dialog;
mod launch_env;
mod login;
mod palette;
mod palette_view;
mod session;
mod slash;
mod status;
mod theme;
mod tool_card;
mod settings;
mod workspace;

use std::path::PathBuf;

use gpui_kit::component::TitleBar;
use gpui_kit::*;

fn main() {
	// Before anything else: it may `set_var` PATH (Finder launches lack it).
	launch_env::import_shell_path();
	let args = Args::parse(std::env::args().skip(1));
	let options = omp_bridge::SessionOptions {
		omp: launch_env::locate_omp(),
		session_dir: args.session_dir,
		..Default::default()
	};
	let reopen_options = options.clone();
	let reopen_cwd = args.cwd.clone();
	let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
	// Dock click / app re-launch while running: reopen a window when none is
	// visible (e.g. the user closed the only window but the app keeps running
	// in the background).
	app.on_reopen(move |cx| {
		if cx.windows().is_empty() {
			open_main_window(reopen_options.clone(), reopen_cwd.clone(), cx);
		}
		cx.activate(true);
	});
	app.run(move |cx| {
		gpui_kit::init(cx);
		theme::apply(cx);
		workspace::init_menus(cx);
		open_main_window(options, args.cwd, cx);
	});
}

/// Open the main workspace window with a single new-session tab.
fn open_main_window(options: omp_bridge::SessionOptions, cwd: PathBuf, cx: &mut App) {
	let bounds = Bounds::centered(None, size(px(1280.), px(840.)), cx);
	let window = WindowOptions {
		window_bounds: Some(WindowBounds::Windowed(bounds)),
		window_min_size: Some(size(px(900.), px(560.))),
		..TitleBar::window_options()
	};
	gpui_kit::open_window(window, cx, move |window, cx| {
		cx.new(|cx| workspace::Workspace::new(options, cwd, window, cx))
	})
	.expect("open window");
	cx.activate(true);
}

struct Args {
	cwd:         PathBuf,
	session_dir: Option<PathBuf>,
}

impl Args {
	fn parse(mut args: impl Iterator<Item = String>) -> Self {
		let mut cwd = None;
		let mut session_dir = None;
		while let Some(arg) = args.next() {
			match arg.as_str() {
				"--cwd" => cwd = args.next().map(PathBuf::from),
				"--session-dir" => session_dir = args.next().map(PathBuf::from),
				_ => {},
			}
		}
		Self {
			cwd: cwd.unwrap_or_else(launch_env::default_cwd),
			session_dir,
		}
	}
}

#[cfg(test)]
mod ui_tests;
