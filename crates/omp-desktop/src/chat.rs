//! The main window: conversation stream, composer, status bar, and the
//! dialog overlay for one omp session.

use std::{path::PathBuf, sync::Arc};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use omp_bridge::{Session, SessionOptions};
use omp_rpc::{
	AbortCommand, Client, Event, GetSessionStatsCommand, GetStateCommand, PromoteQueuedMessageCommand,
	PromptCommand, QueuedMessageQueue, RemoveQueuedMessageCommand, StreamingBehavior,
};
use omp_store::display::{Block, Role, View};

use crate::dialog::{self, DialogEvent, DialogView};
use crate::session::{Activity, Notice, SessionModel};
use crate::theme;
use crate::tool_card::{ToolCard, clip_lines, truncate};

#[derive(Clone, Copy)]
enum Delivery {
	Normal,
	Steer,
}

enum QueueAction {
	Remove(String, QueuedMessageQueue),
	Promote(String),
}

enum Connection {
	Connecting,
	Connected(Arc<Client>),
	/// Startup failed or the process went away.
	Failed(String),
}

pub struct ChatView {
	cwd:        PathBuf,
	options:    SessionOptions,
	connection: Connection,
	model:      SessionModel,
	cost:       Option<f64>,
	list:       ListState,
	composer:   Entity<TextareaState>,
	dialog:     Option<Entity<DialogView>>,
	/// Thinking blocks the user expanded, by (row, block).
	expanded:   std::collections::HashSet<(usize, usize)>,
	/// Abort requests sent, for tests that check one key press aborts once.
	#[cfg(test)]
	pub(crate) aborts: usize,
	_tasks:     Vec<Task<()>>,
	_subs:      Vec<Subscription>,
}

impl ChatView {
	pub fn new(options: SessionOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let cwd = options.cwd.clone().unwrap_or_else(|| PathBuf::from("."));
		let composer = cx.new(|cx| {
			TextareaState::new(window, cx)
				.placeholder("给 omp 发消息…（Enter 发送，Shift+Enter 换行；运行中 Enter 排队，⌘Enter 插队）")
				.auto_grow(2, 10)
				.submit_on_enter(true)
		});
		composer.update(cx, |state, cx| state.focus(window, cx));
		let sub = cx.subscribe_in(&composer, window, |this, _, event, window, cx| match event {
			InputEvent::PressEnter { secondary: true, .. } => this.send(Delivery::Steer, window, cx),
			InputEvent::PressEnter { shift: false, .. } => this.send(Delivery::Normal, window, cx),
			_ => {},
		});
		let list = ListState::new(0, ListAlignment::Top, px(600.));
		list.set_follow_mode(FollowMode::Tail);

		let mut view = Self {
			cwd,
			options,
			connection: Connection::Connecting,
			model: SessionModel::default(),
			cost: None,
			list,
			composer,
			dialog: None,
			expanded: Default::default(),
			#[cfg(test)]
			aborts: 0,
			_tasks: Vec::new(),
			_subs: vec![sub],
		};
		view.connect(cx);
		view
	}

	fn connect(&mut self, cx: &mut Context<Self>) {
		self.connection = Connection::Connecting;
		let options = self.options.clone();
		let spawn = cx.background_spawn(async move { Session::spawn(&options) });
		let task = cx.spawn(async move |this, cx| {
			let session = match spawn.await {
				Ok(session) => session,
				Err(error) => {
					_ = this.update(cx, |this, cx| {
						this.connection = Connection::Failed(error.to_string());
						cx.notify();
					});
					return;
				},
			};
			let events = session.events.clone();
			let client = session.client.clone();
			_ = this.update(cx, |this, cx| {
				this.connection = Connection::Connected(client);
				this.refresh_state(cx);
				cx.notify();
			});
			while let Ok(event) = events.recv().await {
				let Event::Notification(notification) = event else {
					continue;
				};
				let alive = this.update(cx, |this, cx| this.on_notification(notification, cx));
				if alive.is_err() {
					return;
				}
			}
			_ = this.update(cx, |this, cx| {
				this.connection = Connection::Failed("omp 进程已退出".into());
				this.model.activity = Default::default();
				cx.notify();
			});
		});
		self._tasks.push(task);
	}

	fn client(&self) -> Option<Arc<Client>> {
		match &self.connection {
			Connection::Connected(client) => Some(client.clone()),
			_ => None,
		}
	}

	#[cfg(test)]
	pub(crate) fn is_connected(&self) -> bool {
		matches!(self.connection, Connection::Connected(_))
	}

	#[cfg(test)]
	pub(crate) fn is_idle(&self) -> bool {
		!self.model.busy()
	}

	#[cfg(test)]
	pub(crate) fn transcript(&self) -> &omp_store::Transcript {
		&self.model.transcript
	}

	#[cfg(test)]
	pub(crate) fn composer_text(&self, cx: &App) -> String {
		self.composer.read(cx).value().to_string()
	}

	#[cfg(test)]
	pub(crate) fn composer(&self) -> Entity<TextareaState> {
		self.composer.clone()
	}

	/// Labels of the open `select` dialog, if one is shown.
	#[cfg(test)]
	pub(crate) fn open_dialog_options(&self, cx: &App) -> Option<Vec<String>> {
		self.dialog.as_ref()?.read(cx).select_options()
	}

	#[cfg(test)]
	pub(crate) fn open_ask_labels(&self, cx: &App) -> Option<Vec<String>> {
		self.dialog.as_ref()?.read(cx).ask_labels()
	}

	#[cfg(test)]
	pub(crate) fn queue(&self) -> &crate::session::Queue {
		&self.model.queue
	}

	#[cfg(test)]
	pub(crate) fn tool_runs(&self) -> &omp_store::tools::ToolRuns {
		&self.model.tools
	}

	fn on_notification(&mut self, notification: omp_rpc::RpcNotification, cx: &mut Context<Self>) {
		let effects = self.model.apply(notification);
		if let Some((first, count)) = effects.rows {
			let old = self.list.item_count();
			if count != old {
				self.list.splice(old.min(first)..old, count - old.min(first));
			} else {
				self.list.remeasure_items(first..count);
			}
		}
		if effects.refresh_state {
			self.refresh_state(cx);
		}
		self.show_next_dialog(cx);
		cx.notify();
	}

	fn refresh_state(&mut self, cx: &mut Context<Self>) {
		let Some(client) = self.client() else {
			return;
		};
		let fetch = cx.background_spawn(async move {
			(client.call(&GetStateCommand {}).ok(), client.call(&GetSessionStatsCommand {}).ok())
		});
		cx.spawn(async move |this, cx| {
			let (state, stats) = fetch.await;
			_ = this.update(cx, |this, cx| {
				if state.is_some() {
					this.model.state = state;
				}
				if let Some(stats) = stats {
					this.cost = Some(stats.cost);
				}
				cx.notify();
			});
		})
		.detach();
	}

	fn show_next_dialog(&mut self, cx: &mut Context<Self>) {
		if self.dialog.is_some() {
			// The open dialog may have been cancelled by the server.
			let open = self.dialog.as_ref().map(|d| d.read(cx).id().to_owned()).unwrap_or_default();
			if self.model.dialogs.iter().any(|d| crate::session::dialog_id(d) == Some(open.as_str())) {
				return;
			}
			self.dialog = None;
		}
		let Some(request) = self.model.dialogs.first().cloned() else {
			return;
		};
		let Some(window) = cx.windows().first().copied() else {
			return;
		};
		let this = cx.entity().downgrade();
		_ = window.update(cx, |_, window, cx| {
			let view = cx.new(|cx| DialogView::new(request, window, cx));
			let sub = cx.subscribe(&view, move |dialog, event: &DialogEvent, cx| {
				let DialogEvent::Answered(response) = event;
				let id = dialog.read(cx).id().to_owned();
				let response = response.clone();
				_ = this.update(cx, |this, cx| this.answer_dialog(&id, response, cx));
			});
			_ = (view.clone(), sub).1.detach();
			self.dialog = Some(view);
		});
	}

	fn answer_dialog(&mut self, id: &str, response: omp_rpc::ExtensionUiResponse, cx: &mut Context<Self>) {
		if let Some(client) = self.client()
			&& let Err(error) = client.send(&dialog::inbound(response))
		{
			self.model.notices.push(Notice::Error(format!("回复对话框失败：{error}")));
		}
		self.model.dialog_answered(id);
		self.dialog = None;
		self.show_next_dialog(cx);
		cx.notify();
	}

	fn send(&mut self, delivery: Delivery, window: &mut Window, cx: &mut Context<Self>) {
		let text = self.composer.read(cx).value().trim().to_owned();
		let Some(client) = self.client() else {
			return;
		};
		if text.is_empty() {
			return;
		}
		// While the agent works, a message queues: as a follow-up for the next
		// turn, or as a steer injected at the next tool boundary.
		let streaming_behavior = self.model.busy().then_some(match delivery {
			Delivery::Normal => StreamingBehavior::FollowUp,
			Delivery::Steer => StreamingBehavior::Steer,
		});
		self.composer.update(cx, |state, cx| state.clean(window, cx));
		self.model.prompt_sent();
		let call = cx.background_spawn(async move {
			client.call(&PromptCommand { message: text, images: None, streaming_behavior })
		});
		cx.spawn(async move |this, cx| {
			if let Err(error) = call.await {
				_ = this.update(cx, |this, cx| {
					this.model.notices.push(Notice::Error(format!("发送失败：{error}")));
					this.model.activity = Default::default();
					cx.notify();
				});
			}
		})
		.detach();
		cx.notify();
	}

	fn abort(&mut self, cx: &mut Context<Self>) {
		let Some(client) = self.client() else {
			return;
		};
		#[cfg(test)]
		{
			self.aborts += 1;
		}
		cx.background_spawn(async move {
			_ = client.call(&AbortCommand {});
		})
		.detach();
	}

	/// Esc in an empty composer stops the agent; with text it keeps the
	/// input's own handling. Text an IME is composing is already part of the
	/// value, so Esc that cancels a composition does not abort.
	fn on_escape(&mut self, cx: &mut Context<Self>) {
		if self.model.busy() && self.composer.read(cx).value().is_empty() {
			self.abort(cx);
		}
	}

	fn queue_action(&mut self, action: QueueAction, cx: &mut Context<Self>) {
		let Some(client) = self.client() else {
			return;
		};
		let call = cx.background_spawn(async move {
			match action {
				QueueAction::Remove(message, queue) => {
					client.call(&RemoveQueuedMessageCommand { message, queue }).map(|r| r.removed)
				},
				QueueAction::Promote(message) => {
					client.call(&PromoteQueuedMessageCommand { message }).map(|r| r.promoted)
				},
			}
		});
		cx.spawn(async move |this, cx| {
			// The queue itself updates through `queue_update`; only report
			// failures and no-ops (already delivered).
			let notice = match call.await {
				Ok(true) => return,
				Ok(false) => Notice::Info("这条消息已经发出，无法再修改".into()),
				Err(error) => Notice::Error(format!("队列操作失败：{error}")),
			};
			_ = this.update(cx, |this, cx| {
				this.model.notices.push(notice);
				cx.notify();
			});
		})
		.detach();
	}

	fn dismiss_notice(&mut self, index: usize, cx: &mut Context<Self>) {
		if index < self.model.notices.len() {
			self.model.notices.remove(index);
		}
		cx.notify();
	}

	fn render_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
		let p = theme::palette(cx);
		let Some(entry) = self.model.transcript.entries().get(index) else {
			return div().into_any_element();
		};
		let view = entry.view();
		let content = div().w_full().max_w(px(920.)).mx_auto().px_4();
		match &view.role {
			Role::Hidden => div().into_any_element(),
			Role::User => content
				.py_2()
				.flex()
				.justify_end()
				.child(
					div()
						.max_w(px(680.))
						.px_3()
						.py_2()
						.rounded(px(8.))
						.bg(rgb(p.accent))
						.text_sm()
						.children(view.blocks.iter().map(|block| match block {
							Block::Text(text) => div().child(text.clone()).into_any_element(),
							Block::Image { mime_type } => {
								div().text_xs().child(format!("[图片 {mime_type}]")).into_any_element()
							},
							_ => div().into_any_element(),
						})),
				)
				.into_any_element(),
			Role::Assistant => content
				.py_2()
				.flex()
				.flex_col()
				.gap_2()
				.children(view.blocks.iter().enumerate().map(|(bi, block)| self.render_block(index, bi, block, &view, cx)))
				.when(view.streaming && view.blocks.is_empty(), |col| {
					col.child(div().text_sm().text_color(rgb(p.muted_foreground)).child("思考中…"))
				})
				.when_some(view.error.clone(), |col, error| {
					col.child(div().text_sm().text_color(rgb(p.danger)).child(format!("错误：{error}")))
				})
				.into_any_element(),
			// Shown inside the card of the call that produced it; a result with
			// no matching call (history from an older run) falls back here.
			Role::ToolResult { tool_call_id, .. } if self.model.tool_rows.contains_key(tool_call_id) => {
				div().into_any_element()
			},
			Role::ToolResult { tool_name, is_error, .. } => {
				let output: String = view
					.blocks
					.iter()
					.filter_map(|block| match block {
						Block::Output(text) | Block::Text(text) => Some(text.as_str()),
						_ => None,
					})
					.collect::<Vec<_>>()
					.join("\n");
				let (preview, more) = clip_lines(&output, 12);
				content
					.py_1()
					.child(
						div()
							.rounded(px(8.))
							.border_1()
							.border_color(if *is_error { rgb(p.danger) } else { rgb(p.border) })
							.bg(rgb(p.code))
							.px_3()
							.py_2()
							.font_family("Menlo")
							.text_xs()
							.child(div().text_color(rgb(p.muted_foreground)).child(tool_name.clone()))
							.child(preview)
							.when(more > 0, |d| {
								d.child(div().text_color(rgb(p.muted_foreground)).child(format!("… 还有 {more} 行")))
							}),
					)
					.into_any_element()
			},
		}
	}

	fn render_block(&self, row: usize, bi: usize, block: &Block, view: &View, cx: &mut Context<Self>) -> AnyElement {
		let p = theme::palette(cx);
		match block {
			Block::Text(text) => div()
				.max_w(px(680.))
				.text_sm()
				.child(TextView::markdown(ElementId::NamedInteger("md".into(), (row * 64 + bi) as u64), text.clone()))
				.into_any_element(),
			Block::Thinking(text) => {
				let open = self.expanded.contains(&(row, bi));
				let live = view.streaming && bi + 1 == view.blocks.len();
				let summary = if live { "思考中…" } else { "思考过程" };
				div()
					.id(("thinking", row * 64 + bi))
					.max_w(px(680.))
					.rounded(px(6.))
					.border_1()
					.border_color(rgb(p.thinking_border))
					.bg(rgb(p.thinking_bg))
					.child(
						div()
							.id(("thinking-toggle", row * 64 + bi))
							.px_3()
							.py_1()
							.text_xs()
							.text_color(rgb(p.muted_foreground))
							.cursor_pointer()
							.child(format!("{} {}", if open { "▾" } else { "▸" }, summary))
							.on_click(cx.listener(move |this, _, _, cx| {
								if !this.expanded.remove(&(row, bi)) {
									this.expanded.insert((row, bi));
								}
								this.list.remeasure_items(row..row + 1);
								cx.notify();
							})),
					)
					.when(open, |d| {
						d.child(div().px_3().pb_2().text_xs().text_color(rgb(p.muted_foreground)).child(text.clone()))
					})
					.into_any_element()
			},
			Block::ToolCall { id, name, arguments, intent } => {
				let key = row * 64 + bi;
				let expanded = self.expanded.contains(&(row, bi));
				ToolCard {
					id: key,
					name,
					args: arguments,
					intent: intent.as_deref(),
					run: self.model.tools.get(id),
					expanded,
				}
				.render(p, cx.listener(move |this, _, _, cx| {
					if !this.expanded.remove(&(row, bi)) {
						this.expanded.insert((row, bi));
					}
					this.list.remeasure_items(row..row + 1);
					cx.notify();
				}))
			},
			Block::Image { mime_type } => {
				div().text_xs().text_color(rgb(p.muted_foreground)).child(format!("[图片 {mime_type}]")).into_any_element()
			},
			Block::Output(text) => div().font_family("Menlo").text_xs().child(text.clone()).into_any_element(),
		}
	}

	fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let (dot, label) = match (&self.connection, self.model.activity()) {
			(Connection::Connecting, _) => (p.warning, "连接中".to_owned()),
			(Connection::Failed(_), _) => (p.danger, "已断开".to_owned()),
			(Connection::Connected(_), Activity::Idle) => (p.success, "就绪".to_owned()),
			(Connection::Connected(_), Activity::Running) => (p.primary, "运行中".to_owned()),
			(Connection::Connected(_), Activity::Settling) => (p.primary, "收尾中".to_owned()),
		};
		let state = self.model.state.as_ref();
		let model = state.and_then(|s| s.model.as_ref()).map(|m| m.id.clone()).unwrap_or_default();
		let thinking = state.and_then(|s| s.thinking_level).map(|t| format!("思考 {}", t.as_str())).unwrap_or_default();
		let context = state
			.and_then(|s| s.context_usage.as_ref())
			.map(|u| format!("上下文 {:.0}%", u.percent))
			.unwrap_or_default();
		let cost = self.cost.map(|c| format!("${c:.3}")).unwrap_or_default();
		div()
			.h(px(26.))
			.flex_none()
			.flex()
			.items_center()
			.gap_4()
			.px_3()
			.border_t_1()
			.border_color(rgb(p.border))
			.bg(rgb(p.panel))
			.text_xs()
			.text_color(rgb(p.muted_foreground))
			.child(div().flex().items_center().gap_1().child(div().size(px(7.)).rounded_full().bg(rgb(dot))).child(label))
			.child(self.cwd.display().to_string())
			.child(div().flex_1())
			.when_some(self.model.status.clone(), |row, status| row.child(status))
			.child(model)
			.child(thinking)
			.child(context)
			.child(cost)
	}

	fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let busy = self.model.busy();
		let connected = matches!(self.connection, Connection::Connected(_));
		let queue = &self.model.queue;
		let chip = |kind: &'static str, color: u32, text: &String, index: usize, queue: QueuedMessageQueue| {
			let (remove, promote) = (text.clone(), text.clone());
			let follow_up = queue == QueuedMessageQueue::FollowUp;
			div()
				.id(("queue-chip", index))
				.flex()
				.items_center()
				.gap_1()
				.pl_2()
				.pr_1()
				.h(px(26.))
				.rounded(px(6.))
				.border_1()
				.border_color(rgb(p.border))
				.bg(rgb(p.elevated))
				.text_xs()
				.child(div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(color)).child(kind))
				.child(truncate(text, 60))
				.when(follow_up, |chip| {
					chip.child(
						Button::new(("queue-promote", index))
							.ghost()
							.xsmall()
							.label("插队")
							.tooltip("转为 steer，在下一个工具边界注入")
							.on_click(cx.listener(move |this, _, _, cx| {
								this.queue_action(QueueAction::Promote(promote.clone()), cx)
							})),
					)
				})
				.child(Button::new(("queue-remove", index)).ghost().xsmall().label("×").tooltip("撤回").on_click(
					cx.listener(move |this, _, _, cx| this.queue_action(QueueAction::Remove(remove.clone(), queue), cx)),
				))
		};
		let steering = queue.steering.len();
		let chips: Vec<_> = queue
			.steering
			.iter()
			.enumerate()
			.map(|(i, text)| chip("插队", p.warning, text, i, QueuedMessageQueue::Steering))
			.chain(
				queue
					.follow_up
					.iter()
					.enumerate()
					.map(|(i, text)| chip("排队", p.accent_foreground, text, steering + i, QueuedMessageQueue::FollowUp)),
			)
			.collect();
		div()
			.flex_none()
			.w_full()
			.max_w(px(920.))
			.mx_auto()
			.px_4()
			.pb_3()
			.flex()
			.flex_col()
			.gap_2()
			.child(div().flex().flex_wrap().gap_1().children(chips))
			.child(
				div()
					.flex()
					.items_end()
					.gap_2()
					.child(
						div()
							.flex_1()
							.capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
								if event.keystroke.key == "escape" {
									this.on_escape(cx);
								}
							}))
							.child(div().id("composer").child(Textarea::new(&self.composer).w_full()).test_support()),
					)
					.when(busy, |row| {
						row.child(
							Button::new("abort")
								.danger()
								.label("停止")
								.tooltip("Esc（输入框为空时）")
								.on_click(cx.listener(|this, _, _, cx| this.abort(cx))),
						)
					})
					.child(
						Button::new("send")
							.primary()
							.label(if busy { "排队" } else { "发送" })
							.disabled(!connected)
							.on_click(cx.listener(|this, _, window, cx| this.send(Delivery::Normal, window, cx))),
					),
			)
	}

	fn render_notices(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		div()
			.absolute()
			.top_3()
			.right_3()
			.w(px(360.))
			.flex()
			.flex_col()
			.gap_2()
			.children(self.model.notices.iter().enumerate().map(|(index, notice)| {
				let (color, text) = match notice {
					Notice::Info(text) => (p.accent_foreground, text.clone()),
					Notice::Error(text) => (p.danger, text.clone()),
				};
				div()
					.id(("notice", index))
					.px_3()
					.py_2()
					.rounded(px(8.))
					.border_1()
					.border_color(rgb(color))
					.bg(rgb(p.elevated))
					.text_xs()
					.shadow_md()
					.cursor_pointer()
					.child(text)
					.on_click(cx.listener(move |this, _, _, cx| this.dismiss_notice(index, cx)))
			}))
	}

	fn render_failed(&self, error: &str, cx: &mut Context<Self>) -> AnyElement {
		let p = theme::palette(cx);
		div()
			.flex_1()
			.flex()
			.flex_col()
			.items_center()
			.justify_center()
			.gap_3()
			.child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("无法连接 omp"))
			.child(div().max_w(px(560.)).text_sm().text_color(rgb(p.muted_foreground)).child(error.to_owned()))
			.child(
				div()
					.font_family("Menlo")
					.text_xs()
					.px_3()
					.py_2()
					.rounded(px(6.))
					.bg(rgb(p.code))
					.child("brew install can1357/tap/omp"),
			)
			.child(Button::new("reconnect").primary().label("重新连接").on_click(cx.listener(|this, _, _, cx| {
				this.model = SessionModel::default();
				this.list.reset(0);
				this.connect(cx);
				cx.notify();
			})))
			.into_any_element()
	}
}

impl Render for ChatView {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let body = match &self.connection {
			Connection::Failed(error) if self.model.row_count() == 0 => self.render_failed(&error.clone(), cx),
			_ if self.model.row_count() == 0 => div()
				.flex_1()
				.flex()
				.flex_col()
				.items_center()
				.justify_center()
				.gap_2()
				.child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("开始一个新会话"))
				.child(div().text_sm().text_color(rgb(p.muted_foreground)).child(format!("工作目录 {}", self.cwd.display())))
				.child(
					div()
						.text_xs()
						.text_color(rgb(p.muted_foreground))
						.child(if matches!(self.connection, Connection::Connecting) { "正在启动 omp…" } else { "输入消息后按 Enter 发送" }),
				)
				.into_any_element(),
			_ => {
				let this = cx.entity().downgrade();
				div()
					.flex_1()
					.min_h_0()
					.child(
						list(self.list.clone(), move |index, _window, cx| {
							this.upgrade()
								.map(|view| view.update(cx, |view, cx| view.render_row(index, cx)))
								.unwrap_or_else(|| div().into_any_element())
						})
						.size_full(),
					)
					.into_any_element()
			},
		};
		let disconnected_banner = match &self.connection {
			Connection::Failed(error) if self.model.row_count() > 0 => Some(error.clone()),
			_ => None,
		};

		div()
			.size_full()
			.relative()
			.flex()
			.flex_col()
			.bg(rgb(p.background))
			.text_color(cx.theme().foreground)
			.when_some(disconnected_banner, |root, error| {
				root.child(
					div()
						.flex()
						.items_center()
						.gap_3()
						.px_4()
						.py_2()
						.bg(rgb(p.danger))
						.text_color(rgb(0xFFFFFF))
						.text_sm()
						.child(format!("已断开：{error}"))
						.child(Button::new("reconnect-banner").small().label("重新连接").on_click(cx.listener(|this, _, _, cx| {
							this.model = SessionModel::default();
							this.list.reset(0);
							this.connect(cx);
							cx.notify();
						}))),
				)
			})
			.child(body)
			.child(self.render_composer(cx))
			.child(self.render_status(cx))
			.child(self.render_notices(cx))
			.when_some(self.dialog.clone(), |root, dialog| {
				root.child(
					div()
						.absolute()
						.inset_0()
						.flex()
						.items_center()
						.justify_center()
						.bg(hsla(0., 0., 0., 0.35))
						.child(dialog),
				)
			})
	}
}
