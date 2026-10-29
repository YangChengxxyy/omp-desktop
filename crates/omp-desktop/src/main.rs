//! omp-desktop entry point: one window, one omp session in the launch
//! directory (or `--cwd <dir>`).

mod chat;
mod dialog;
mod session;
mod theme;
mod tool_card;

use std::path::PathBuf;

use gpui_kit::*;

fn main() {
	let options = omp_bridge::SessionOptions { cwd: Some(parse_cwd()), ..Default::default() };
	gpui_kit::application().run(move |cx| {
		gpui_kit::init(cx);
		theme::apply(cx);
		let bounds = Bounds::centered(None, size(px(1180.), px(820.)), cx);
		let window = WindowOptions {
			window_bounds: Some(WindowBounds::Windowed(bounds)),
			titlebar: Some(TitlebarOptions { title: Some("omp".into()), ..Default::default() }),
			window_min_size: Some(size(px(720.), px(480.))),
			..Default::default()
		};
		gpui_kit::open_window(window, cx, |window, cx| cx.new(|cx| chat::ChatView::new(options, window, cx)))
			.expect("open window");
		cx.activate(true);
	});
}

/// `--cwd <dir>` overrides the launch directory.
fn parse_cwd() -> PathBuf {
	let mut args = std::env::args().skip(1);
	while let Some(arg) = args.next() {
		if arg == "--cwd"
			&& let Some(dir) = args.next()
		{
			return PathBuf::from(dir);
		}
	}
	std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod ui_tests;
