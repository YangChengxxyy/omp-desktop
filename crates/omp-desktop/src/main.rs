//! omp-desktop entry point: one window with session tabs, starting with a new
//! session in the launch directory (or `--cwd <dir>`). `--session-dir <dir>`
//! stores and lists sessions there instead of `~/.omp/agent/sessions`.

mod accounts;
mod attachments;
mod chat;
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
	gpui_kit::application().run(move |cx| {
		gpui_kit::init(cx);
		theme::apply(cx);
		let bounds = Bounds::centered(None, size(px(1280.), px(840.)), cx);
		let window = WindowOptions {
			window_bounds: Some(WindowBounds::Windowed(bounds)),
			window_min_size: Some(size(px(900.), px(560.))),
			..TitleBar::window_options()
		};
		let cwd = args.cwd.clone();
		gpui_kit::open_window(window, cx, |window, cx| {
			cx.new(|cx| workspace::Workspace::new(options, cwd, window, cx))
		})
		.expect("open window");
		cx.activate(true);
	});
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
