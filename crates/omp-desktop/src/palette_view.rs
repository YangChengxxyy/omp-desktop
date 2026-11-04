//! The command palette overlay. Entries come from the workspace; picking one
//! emits it back. ↑↓ select, Enter runs, Esc (or a click outside) closes.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::palette::{self, Command, Entry};
use crate::theme;

pub enum PaletteEvent {
	Run(Command),
	Close,
}

impl EventEmitter<PaletteEvent> for PaletteView {}

pub struct PaletteView {
	entries:  Vec<Entry>,
	query:    Entity<InputState>,
	selected: usize,
	scroll:   ScrollHandle,
	_sub:     Subscription,
}

impl PaletteView {
	pub fn new(entries: Vec<Entry>, placeholder: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let query = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_owned()));
		query.update(cx, |state, cx| state.focus(window, cx));
		let sub = cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
			if matches!(event, InputEvent::Change) {
				this.selected = 0;
				this.scroll.scroll_to_item(0);
				cx.notify();
			}
		});
		// Start on the current choice when there is one (⌘M).
		let selected = entries.iter().position(|e| e.current && e.group != palette::Group::Action).unwrap_or(0);
		let scroll = ScrollHandle::new();
		scroll.scroll_to_item(selected);
		Self { entries, query, selected, scroll, _sub: sub }
	}

	fn matches(&self, cx: &App) -> Vec<&Entry> {
		palette::rank(&self.entries, &self.query.read(cx).value())
	}

	fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
		let len = self.matches(cx).len();
		if len == 0 {
			return;
		}
		self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
		self.scroll.scroll_to_item(self.selected);
		cx.notify();
	}

	fn run(&mut self, index: usize, cx: &mut Context<Self>) {
		if let Some(entry) = self.matches(cx).get(index) {
			let command = entry.command.clone();
			cx.emit(PaletteEvent::Run(command));
		}
	}

	#[cfg(test)]
	pub(crate) fn titles(&self, cx: &App) -> Vec<String> {
		self.matches(cx).into_iter().map(|e| e.title.clone()).collect()
	}

	#[cfg(test)]
	pub(crate) fn selected(&self) -> usize {
		self.selected
	}
}

impl Render for PaletteView {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let selected = self.selected;
		let rows: Vec<AnyElement> = self
			.matches(cx)
			.into_iter()
			.enumerate()
			.map(|(index, entry)| {
				let primary = entry.subtitle.as_ref().unwrap_or(&entry.title).clone();
				let secondary = entry.subtitle.as_ref().map(|_| entry.title.clone());
				div()
					.id(("palette-row", index))
					.flex()
					.items_center()
					.gap_2()
					.px_3()
					.h(px(34.))
					.rounded(px(6.))
					.text_sm()
					.cursor_pointer()
					.when(index == selected, |r| r.bg(rgb(p.accent)).text_color(rgb(p.accent_foreground)))
					.when(index != selected, |r| r.hover(|s| s.bg(rgb(p.muted))))
					.child(div().w(px(36.)).flex_none().text_xs().text_color(rgb(p.muted_foreground)).child(entry.group.label()))
					.child(div().flex_none().max_w(px(240.)).truncate().font_weight(FontWeight::MEDIUM).child(primary))
					.when_some(secondary.clone(), |r, id| {
						r.child(div().flex_1().min_w_0().truncate().text_xs().text_color(rgb(p.muted_foreground)).child(id))
					})
					.when(secondary.is_none() && entry.provider.is_some(), |r| r.child(div().flex_1()))
					.when(entry.current, |r| r.child(div().flex_none().text_xs().text_color(rgb(p.success)).child("当前")))
					.when(entry.provider.is_none(), |r| {
						r.child(div().flex_1().min_w_0().truncate().text_xs().text_color(rgb(p.muted_foreground)).child(entry.detail.clone()))
					})
					.when_some(entry.provider.clone(), |r, provider| {
						r.child(
							div()
								.flex_none()
								.px_1p5()
								.py_0p5()
								.rounded(px(4.))
								.bg(rgb(p.muted))
								.text_xs()
								.text_color(rgb(p.muted_foreground))
								.child(provider),
						)
					})
					.when_some(entry.shortcut, |r, keys| {
						r.child(div().flex_none().text_xs().font_family("Menlo").text_color(rgb(p.muted_foreground)).child(keys))
					})
					.on_click(cx.listener(move |this, _, _, cx| this.run(index, cx)))
					.test_support()
					.into_any_element()
			})
			.collect();
		let empty = rows.is_empty();
		div()
			.id("palette")
			.w(px(640.))
			.max_h(px(480.))
			.flex()
			.flex_col()
			.rounded(px(10.))
			.border_1()
			.border_color(rgb(p.border))
			.bg(rgb(p.elevated))
			.shadow_lg()
			.overflow_hidden()
			// Clicks inside must not reach the overlay's close handler.
			.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
			.capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
				this.step(-1, cx);
				cx.stop_propagation();
			}))
			.capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
				this.step(1, cx);
				cx.stop_propagation();
			}))
			.capture_action(cx.listener(|this, _: &Enter, _, cx| {
				let index = this.selected;
				this.run(index, cx);
				cx.stop_propagation();
			}))
			.capture_action(cx.listener(|_, _: &Escape, _, cx| {
				cx.emit(PaletteEvent::Close);
				cx.stop_propagation();
			}))
			.child(div().p_2().border_b_1().border_color(rgb(p.border)).child(Input::new(&self.query).appearance(false)))
			.child(
				div()
					.id("palette-list")
					.flex_1()
					.min_h_0()
					.overflow_y_scroll()
					.track_scroll(&self.scroll)
					.p_1()
					.children(rows)
					.when(empty, |list| {
						list.child(div().p_3().text_sm().text_color(rgb(p.muted_foreground)).child("没有匹配的命令"))
					}),
			)
			.test_support()
	}
}
