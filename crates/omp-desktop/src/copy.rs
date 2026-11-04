//! Copy buttons shared by the transcript: messages (Markdown source), code
//! blocks, tool commands and their output. Each shows a check for two seconds
//! after copying.

use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::text::TextView;
use gpui_kit::component::Sizable as _;
use gpui_kit::*;

/// Group name for rows whose copy button appears on hover.
pub const HOVER_GROUP: &str = "copy-hover";

/// An icon button that copies `value`.
pub fn button(id: impl Into<ElementId>, value: impl Into<SharedString>, tooltip: &'static str) -> Clipboard {
	Clipboard::new(id).value(value).tooltip(tooltip).xsmall()
}

/// Markdown with a copy button in the corner of each fenced code block.
pub fn markdown(id: impl Into<ElementId>, text: impl Into<SharedString>) -> TextView {
	TextView::markdown(id, text).code_block_actions(|block, _, _| {
		let label = block.lang().filter(|lang| !lang.is_empty()).map_or("复制代码".into(), |lang| format!("复制 {lang}"));
		Clipboard::new("copy").value(block.code()).tooltip(label).xsmall()
	})
}
