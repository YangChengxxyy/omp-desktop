//! Modal answers to `extension_ui_request` dialogs (`select`, `confirm`,
//! `input`, `editor`, `ask`). One dialog is shown at a time; the rest wait in
//! `SessionModel::dialogs`.

use std::collections::BTreeSet;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::*;
use omp_rpc::{
	AnswersUiResponse, AskAnswer, CancelUiResponse, ConfirmUiResponse, ExtensionUiRequest,
	ExtensionUiResponse, LitTrue, RpcInbound, ValueUiResponse,
};

use crate::theme;

/// The answer the user gave, ready to send.
pub fn inbound(response: ExtensionUiResponse) -> RpcInbound {
	RpcInbound::ExtensionUiResponse(response)
}

pub fn cancel(id: &str) -> ExtensionUiResponse {
	ExtensionUiResponse::CancelUiResponse(CancelUiResponse { id: id.to_owned(), cancelled: LitTrue, timed_out: None })
}

pub enum DialogEvent {
	Answered(ExtensionUiResponse),
}

impl EventEmitter<DialogEvent> for DialogView {}

pub struct DialogView {
	request:  ExtensionUiRequest,
	/// Text for `input` / `editor`, and per-question free text for `ask`.
	text:     Option<Entity<InputState>>,
	area:     Option<Entity<TextareaState>>,
	/// `ask`: selected option labels per question.
	selected: Vec<BTreeSet<String>>,
	custom:   Vec<Entity<InputState>>,
}

impl DialogView {
	pub fn new(request: ExtensionUiRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let mut view = Self { request, text: None, area: None, selected: Vec::new(), custom: Vec::new() };
		match &view.request {
			ExtensionUiRequest::Input(input) => {
				let placeholder = input.placeholder.clone().unwrap_or_default();
				let secret = input.secret == Some(true);
				let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).masked(secret));
				state.update(cx, |state, cx| state.focus(window, cx));
				view.text = Some(state);
			},
			ExtensionUiRequest::Editor(editor) => {
				let prefill = editor.prefill.clone().unwrap_or_default();
				let state = cx.new(|cx| TextareaState::new(window, cx).rows(8).default_value(prefill));
				state.update(cx, |state, cx| state.focus(window, cx));
				view.area = Some(state);
			},
			ExtensionUiRequest::Ask(ask) => {
				for question in &ask.questions {
					let recommended = question
						.recommended
						.and_then(|index| question.options.get(usize::try_from(index).ok()?))
						.map(|option| option.label.clone());
					view.selected.push(recommended.into_iter().collect());
					view.custom.push(cx.new(|cx| InputState::new(window, cx).placeholder("其他（自由输入）")));
				}
			},
			_ => {},
		}
		view
	}

	pub fn id(&self) -> &str {
		crate::session::dialog_id(&self.request).unwrap_or_default()
	}

	#[cfg(test)]
	pub(crate) fn select_options(&self) -> Option<Vec<String>> {
		match &self.request {
			ExtensionUiRequest::Select(select) => Some(select.options.clone()),
			_ => None,
		}
	}

	/// Option labels of an open `ask` (all questions, in order).
	#[cfg(test)]
	pub(crate) fn ask_labels(&self) -> Option<Vec<String>> {
		match &self.request {
			ExtensionUiRequest::Ask(ask) => {
				Some(ask.questions.iter().flat_map(|q| q.options.iter().map(|o| o.label.clone())).collect())
			},
			_ => None,
		}
	}

	fn answer(&mut self, response: ExtensionUiResponse, cx: &mut Context<Self>) {
		cx.emit(DialogEvent::Answered(response));
	}

	fn submit(&mut self, cx: &mut Context<Self>) {
		let id = self.id().to_owned();
		let response = match &self.request {
			ExtensionUiRequest::Input(_) => {
				let value = self.text.as_ref().map(|t| t.read(cx).value().to_string()).unwrap_or_default();
				ExtensionUiResponse::ValueUiResponse(ValueUiResponse { id, value })
			},
			ExtensionUiRequest::Editor(_) => {
				let value = self.area.as_ref().map(|t| t.read(cx).value().to_string()).unwrap_or_default();
				ExtensionUiResponse::ValueUiResponse(ValueUiResponse { id, value })
			},
			ExtensionUiRequest::Ask(ask) => {
				let answers = ask
					.questions
					.iter()
					.enumerate()
					.map(|(index, question)| {
						let custom = self.custom[index].read(cx).value().trim().to_owned();
						let mut selected: Vec<String> = question
							.options
							.iter()
							.filter(|option| self.selected[index].contains(&option.label))
							.map(|option| option.label.clone())
							.collect();
						// A single-select takes an option or free text, not both.
						if !question.multi && !custom.is_empty() {
							selected.clear();
						}
						AskAnswer {
							id:               question.id.clone(),
							selected_options: selected,
							custom_input:     (!custom.is_empty()).then_some(custom),
						}
					})
					.collect();
				ExtensionUiResponse::AnswersUiResponse(AnswersUiResponse { id, answers })
			},
			_ => return,
		};
		self.answer(response, cx);
	}

	fn toggle(&mut self, question: usize, label: String, multi: bool, cx: &mut Context<Self>) {
		let set = &mut self.selected[question];
		if multi {
			if !set.remove(&label) {
				set.insert(label);
			}
		} else {
			set.clear();
			set.insert(label);
		}
		cx.notify();
	}
}

impl Render for DialogView {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let id = self.id().to_owned();
		let (title, body): (String, AnyElement) = match &self.request {
			ExtensionUiRequest::Confirm(confirm) => (
				confirm.title.clone(),
				div().text_sm().child(confirm.message.clone()).into_any_element(),
			),
			ExtensionUiRequest::Select(select) => {
				let details = select.option_details.clone().unwrap_or_default();
				let options = select.options.iter().enumerate().map(|(index, option)| {
					let description = details.get(index).and_then(|d| d.description.clone());
					let response = ExtensionUiResponse::ValueUiResponse(ValueUiResponse { id: id.clone(), value: option.clone() });
					div()
						.id(("select-option", index))
						.flex()
						.flex_col()
						.px_3()
						.py_2()
						.rounded(px(6.))
						.border_1()
						.border_color(rgb(p.border))
						.cursor_pointer()
						.hover(|style| style.bg(rgb(p.accent)))
						.child(div().text_sm().font_weight(FontWeight::MEDIUM).child(option.clone()))
						.when_some(description, |row, description| {
							row.child(div().text_xs().text_color(rgb(p.muted_foreground)).child(description))
						})
						.on_click(cx.listener(move |this, _, _, cx| this.answer(response.clone(), cx)))
						.test_support()
				});
				(select.title.clone(), div().flex().flex_col().gap_1().children(options).into_any_element())
			},
			ExtensionUiRequest::Input(input) => (
				input.title.clone(),
				Input::new(self.text.as_ref().expect("input state")).into_any_element(),
			),
			ExtensionUiRequest::Editor(editor) => (
				editor.title.clone(),
				Textarea::new(self.area.as_ref().expect("editor state")).into_any_element(),
			),
			ExtensionUiRequest::Ask(ask) => {
				let questions = ask.questions.iter().enumerate().map(|(qi, question)| {
					let multi = question.multi;
					let options = question.options.iter().enumerate().map(|(oi, option)| {
						let checked = self.selected[qi].contains(&option.label);
						let label = option.label.clone();
						let recommended = question.recommended == Some(oi as i64);
						div()
							.id(("ask-option", qi * 1000 + oi))
							.flex()
							.flex_col()
							.px_3()
							.py_2()
							.rounded(px(6.))
							.border_1()
							.border_color(if checked { rgb(p.primary) } else { rgb(p.border) })
							.when(checked, |row| row.bg(rgb(p.accent)))
							.cursor_pointer()
							.child(
								div()
									.flex()
									.gap_2()
									.text_sm()
									.child(if multi {
										if checked { "☑" } else { "☐" }
									} else if checked {
										"◉"
									} else {
										"○"
									})
									.child(div().font_weight(FontWeight::MEDIUM).child(option.label.clone()))
									.when(recommended, |row| {
										row.child(div().text_xs().text_color(rgb(p.accent_foreground)).child("推荐"))
									}),
							)
							.when_some(option.description.clone(), |row, description| {
								row.child(div().pl_5().text_xs().text_color(rgb(p.muted_foreground)).child(description))
							})
							.on_click(cx.listener(move |this, _, _, cx| this.toggle(qi, label.clone(), multi, cx)))
							.test_support()
					});
					div()
						.flex()
						.flex_col()
						.gap_1()
						.when_some(question.header.clone(), |col, header| {
							col.child(div().text_xs().text_color(rgb(p.muted_foreground)).child(header))
						})
						.child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(question.question.clone()))
						.when(multi, |col| col.child(div().text_xs().text_color(rgb(p.muted_foreground)).child("可多选")))
						.children(options)
						.child(div().id(("ask-custom", qi)).child(Input::new(&self.custom[qi]).small()).test_support())
				});
				("需要你的回答".to_owned(), div().flex().flex_col().gap_4().children(questions).into_any_element())
			},
			_ => (String::new(), div().into_any_element()),
		};

		let cancel_id = id.clone();
		let mut actions = div().flex().justify_end().gap_2().child(
			Button::new("dialog-cancel")
				.ghost()
				.label("取消")
				.on_click(cx.listener(move |this, _, _, cx| this.answer(cancel(&cancel_id), cx))),
		);
		actions = match &self.request {
			ExtensionUiRequest::Confirm(_) => {
				let (no, yes) = (id.clone(), id.clone());
				actions
					.child(Button::new("dialog-no").outline().label("否").on_click(cx.listener(move |this, _, _, cx| {
						this.answer(ExtensionUiResponse::ConfirmUiResponse(ConfirmUiResponse { id: no.clone(), confirmed: false }), cx)
					})))
					.child(Button::new("dialog-yes").primary().label("是").on_click(cx.listener(move |this, _, _, cx| {
						this.answer(ExtensionUiResponse::ConfirmUiResponse(ConfirmUiResponse { id: yes.clone(), confirmed: true }), cx)
					})))
			},
			ExtensionUiRequest::Select(_) => actions,
			_ => actions.child(
				Button::new("dialog-submit").primary().label("提交").on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
			),
		};

		div()
			.id("dialog")
			.w(px(560.))
			.max_h(px(620.))
			.flex()
			.flex_col()
			.gap_3()
			.p_4()
			.rounded(px(10.))
			.border_1()
			.border_color(rgb(p.border))
			.bg(rgb(p.elevated))
			.text_color(cx.theme().foreground)
			.shadow_lg()
			.child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(title))
			.child(div().id("dialog-body").flex_1().overflow_y_scroll().child(body))
			.child(actions)
	}
}
