//! One session tab: conversation stream, composer, status bar, and the
//! dialog overlay for one omp process.

use std::{path::PathBuf, sync::Arc};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Paste, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::text::TextView;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use omp_bridge::{Session, SessionOptions};
use omp_rpc::{
	AbortCommand, AgentMessage, CancelSubagentCommand, Client, CompactCommand, Event, ForkCommand,
	GetAvailableModelsCommand, GetAvailableThinkingLevelsCommand, GetSessionStatsCommand, GetStateCommand,
	GetSubagentsCommand, ModelInfo, PromoteQueuedMessageCommand, PromptCommand, QueuedMessageQueue,
	RemoveQueuedMessageCommand, SessionStats, SetAutoCompactionCommand, SetModelCommand, SetThinkingLevelCommand,
	SteerSubagentCommand, StreamingBehavior, SubagentStatus, ThinkingLevel,
};
use omp_store::display::{Block, Role, View};

use crate::attachments;
use crate::dialog::{self, DialogEvent, DialogView};
use crate::session::{Activity, Notice, PendingControl, SessionModel};
use crate::status;
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

/// What a tab shows in the tab bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabStatus {
	Connecting,
	Idle,
	Running,
	/// A dialog waits for the user.
	Attention,
	Disconnected,
}

/// The parts of a tab the workspace renders outside the tab itself.
#[derive(Debug, Clone, PartialEq)]
pub struct TabSummary {
	pub status:       TabStatus,
	pub name:         Option<String>,
	pub session_file: Option<PathBuf>,
	pub cwd:          PathBuf,
}

pub struct ChatView {
	cwd:          PathBuf,
	options:      SessionOptions,
	/// Fork the resumed session right after connecting (`fork` command), so
	/// the tab continues in a new file and the original stays untouched.
	fork:         bool,
	connection:   Connection,
	/// From `get_state`. For a new session the file appears on disk only
	/// after the first message.
	session_file: Option<PathBuf>,
	model:        SessionModel,
	stats:        Option<SessionStats>,
	/// Choices for the status-bar menus, fetched once per connection.
	models:       Vec<ModelInfo>,
	thinking_levels: Vec<ThinkingLevel>,
	/// The right-hand panel (context, cost, todos, subagents, every
	/// `get_state` field) is open.
	panel:        bool,
	/// Steer input per subagent id, created when its 干预 button is pressed.
	steer_inputs: std::collections::HashMap<String, (Entity<InputState>, Subscription)>,
	list:         ListState,
	composer:     Entity<TextareaState>,
	dialog:       Option<Entity<DialogView>>,
	/// Images waiting in the composer, sent with the next message.
	attachments:  Vec<attachments::Attachment>,
	/// Decoded transcript images by (row, block); cleared with the list.
	images:       std::cell::RefCell<std::collections::HashMap<(usize, usize), Option<Arc<Image>>>>,
	/// Thinking blocks the user expanded, by (row, block).
	expanded:     std::collections::HashSet<(usize, usize)>,
	/// Abort requests sent, for tests that check one key press aborts once.
	#[cfg(test)]
	pub(crate) aborts: usize,
	_tasks:       Vec<Task<()>>,
	_subs:        Vec<Subscription>,
}

impl ChatView {
	/// A tab for `options`: a new session, or `options.resume`.
	pub fn new(options: SessionOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
		Self::build(options, false, window, cx)
	}

	/// A tab that resumes `options.resume` and forks it into a new file.
	pub fn fork(options: SessionOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
		Self::build(options, true, window, cx)
	}

	fn build(options: SessionOptions, fork: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let cwd = options.cwd.clone().unwrap_or_else(|| PathBuf::from("."));
		let composer = cx.new(|cx| {
			TextareaState::new(window, cx)
				.placeholder("给 omp 发消息…（Enter 发送，Shift+Enter 换行；运行中 Enter 排队，⌘Enter 插队；⌘V 或拖入可附加图片）")
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
			session_file: options.resume.as_ref().map(|f| std::fs::canonicalize(f).unwrap_or_else(|_| f.clone())),
			options,
			fork,
			connection: Connection::Connecting,
			model: SessionModel::default(),
			stats: None,
			models: Vec::new(),
			thinking_levels: Vec::new(),
			panel: false,
			steer_inputs: Default::default(),
			list,
			composer,
			dialog: None,
			attachments: Vec::new(),
			images: Default::default(),
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
		let fork = self.fork;
		// Spawn, fork, and load history off the UI thread. Events that arrive
		// meanwhile wait in the channel and apply after the history.
		let spawn = cx.background_spawn(async move {
			let session = Session::spawn(&options)?;
			let mut notice = None;
			if fork {
				match session.client.call(&ForkCommand::default()) {
					Ok(result) if result.cancelled => notice = Some(Notice::Error("fork 被扩展取消".into())),
					Ok(_) => {},
					Err(error) => notice = Some(Notice::Error(format!("fork 失败：{error}"))),
				}
			}
			let history: Vec<AgentMessage> =
				if options.resume.is_some() { session.client.get_messages()? } else { Vec::new() };
			Ok::<_, omp_bridge::BridgeError>((session, history, notice))
		});
		let task = cx.spawn(async move |this, cx| {
			let (session, history, notice) = match spawn.await {
				Ok(started) => started,
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
				if !history.is_empty() {
					this.model.load_history(history);
					this.images.borrow_mut().clear();
					this.list.reset(this.model.row_count());
				}
				this.model.notices.extend(notice);
				this.connection = Connection::Connected(client);
				this.refresh_state(cx);
				this.load_choices(cx);
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

	/// Restarts the process after it exited. A session that reached disk
	/// continues from its file (`--resume`); otherwise a new one starts.
	fn reconnect(&mut self, cx: &mut Context<Self>) {
		if let Some(file) = self.session_file.clone().filter(|file| file.is_file()) {
			self.options.resume = Some(file);
		}
		self.fork = false;
		self.model = SessionModel::default();
		self.dialog = None;
		self.images.borrow_mut().clear();
		self.list.reset(0);
		self.connect(cx);
		cx.notify();
	}

	/// Stops the process. Teardown waits for the process group, so the last
	/// client handle is dropped off the UI thread.
	pub fn shutdown(&mut self, cx: &mut Context<Self>) {
		let old = std::mem::replace(&mut self.connection, Connection::Failed("已关闭".into()));
		self._tasks.clear();
		if let Connection::Connected(client) = old {
			cx.background_spawn(async move { drop(client) }).detach();
		}
	}

	pub fn summary(&self) -> TabSummary {
		let status = match &self.connection {
			Connection::Connecting => TabStatus::Connecting,
			Connection::Failed(_) => TabStatus::Disconnected,
			Connection::Connected(_) if self.model.needs_attention() => TabStatus::Attention,
			Connection::Connected(_) if self.model.busy() => TabStatus::Running,
			Connection::Connected(_) => TabStatus::Idle,
		};
		TabSummary {
			status,
			name: self.model.state.as_ref().and_then(|s| s.session_name.clone()).filter(|n| !n.trim().is_empty()),
			session_file: self.session_file.clone(),
			cwd: self.cwd.clone(),
		}
	}

	pub fn focus_composer(&self, window: &mut Window, cx: &mut App) {
		self.composer.update(cx, |state, cx| state.focus(window, cx));
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
	pub(crate) fn is_disconnected(&self) -> bool {
		matches!(self.connection, Connection::Failed(_))
	}

	#[cfg(test)]
	pub(crate) fn pid(&self) -> Option<u32> {
		self.client()?.pid()
	}

	#[cfg(test)]
	pub(crate) fn queue_log(&self) -> &[crate::session::Queue] {
		&self.model.queue_log
	}

	#[cfg(test)]
	pub(crate) fn session_file(&self) -> Option<&std::path::Path> {
		self.session_file.as_deref()
	}

	#[cfg(test)]
	pub(crate) fn notices(&self) -> &[Notice] {
		&self.model.notices
	}

	#[cfg(test)]
	pub(crate) fn open_dialog_kind(&self, cx: &App) -> Option<&'static str> {
		Some(self.dialog.as_ref()?.read(cx).kind())
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
	pub(crate) fn attachment_labels(&self) -> Vec<String> {
		self.attachments.iter().map(attachments::Attachment::label).collect()
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
	pub(crate) fn state(&self) -> Option<&omp_rpc::SessionState> {
		self.model.state.as_ref()
	}

	#[cfg(test)]
	pub(crate) fn subagents(&self) -> &[crate::session::SubagentRow] {
		&self.model.subagents
	}

	#[cfg(test)]
	pub(crate) fn choices(&self) -> (usize, usize) {
		(self.models.len(), self.thinking_levels.len())
	}

	#[cfg(test)]
	pub(crate) fn thinking_levels_for_test(&self) -> Vec<ThinkingLevel> {
		self.thinking_levels.clone()
	}

	#[cfg(test)]
	pub(crate) fn models_for_test(&self) -> Vec<(String, String)> {
		self.models.iter().map(|m| (m.provider.clone(), m.id.clone())).collect()
	}

	#[cfg(test)]
	pub(crate) fn has_stats(&self) -> bool {
		self.stats.is_some()
	}

	#[cfg(test)]
	pub(crate) fn control_pending(&self) -> bool {
		self.model.pending.is_some()
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
				if let Some(state) = state {
					if let Some(file) = &state.session_file {
						let file = PathBuf::from(file);
						// Canonical once the file exists, to match the session list.
						this.session_file = Some(std::fs::canonicalize(&file).unwrap_or(file));
					}
					this.model.state = Some(state);
				}
				if let Some(stats) = stats {
					this.stats = Some(stats);
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
		if text.is_empty() && self.attachments.is_empty() {
			return;
		}
		let images = std::mem::take(&mut self.attachments);
		let wire = (!images.is_empty()).then(|| images.iter().map(attachments::Attachment::to_wire).collect::<Vec<_>>());
		// While the agent works, a message queues: as a follow-up for the next
		// turn, or as a steer injected at the next tool boundary.
		let streaming_behavior = self.model.busy().then_some(match delivery {
			Delivery::Normal => StreamingBehavior::FollowUp,
			Delivery::Steer => StreamingBehavior::Steer,
		});
		self.composer.update(cx, |state, cx| state.clean(window, cx));
		self.model.prompt_sent();
		let call = cx.background_spawn(async move {
			client.call(&PromptCommand { message: text, images: wire, streaming_behavior })
		});
		cx.spawn(async move |this, cx| {
			if let Err(error) = call.await {
				_ = this.update(cx, |this, cx| {
					this.model.notices.push(Notice::Error(format!("发送失败：{error}")));
					this.model.activity = Default::default();
					// Keep the images so the user can resend.
					if this.attachments.is_empty() {
						this.attachments = images;
					}
					cx.notify();
				});
			}
		})
		.detach();
		cx.notify();
	}

	/// Adds pasted or dropped images to the composer; reports what was
	/// skipped. Returns whether the paste carried anything besides text.
	fn attach(&mut self, pasted: attachments::Pasted, cx: &mut Context<Self>) -> bool {
		if pasted.is_empty() {
			return false;
		}
		if !pasted.rejected.is_empty() {
			self.model.notices.push(Notice::Info(format!("未附加：{}", pasted.rejected.join("、"))));
		}
		for image in pasted.images {
			// The same bitmap pasted twice attaches once.
			if !self.attachments.iter().any(|a| a.image.id == image.image.id) {
				self.attachments.push(image);
			}
		}
		cx.notify();
		true
	}

	fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
		if index < self.attachments.len() {
			self.attachments.remove(index);
		}
		cx.notify();
	}

	fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		div().flex().flex_wrap().gap_2().children(self.attachments.iter().enumerate().map(|(index, attachment)| {
			div()
				.id(("attachment", index))
				.relative()
				.flex()
				.flex_col()
				.items_center()
				.gap_0p5()
				.p_1()
				.rounded(px(6.))
				.border_1()
				.border_color(rgb(p.border))
				.bg(rgb(p.elevated))
				.child(
					img(attachment.image.clone())
						.size(px(56.))
						.object_fit(ObjectFit::Cover)
						.rounded(px(4.)),
				)
				.child(
					div()
						.max_w(px(64.))
						.truncate()
						.text_xs()
						.text_color(rgb(p.muted_foreground))
						.child(attachment.label()),
				)
				.child(
					div().absolute().top_0().right_0().child(
						Button::new(("attachment-remove", index))
							.ghost()
							.xsmall()
							.label("×")
							.tooltip("移除")
							.on_click(cx.listener(move |this, _, _, cx| this.remove_attachment(index, cx))),
					),
				)
				.test_support()
		}))
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

	/// Model and thinking-level choices plus the subagent snapshot, which
	/// covers runs that started before this client subscribed (resume,
	/// reconnect).
	fn load_choices(&mut self, cx: &mut Context<Self>) {
		let Some(client) = self.client() else {
			return;
		};
		let fetch = cx.background_spawn(async move {
			(
				client.call(&GetAvailableModelsCommand {}).ok(),
				client.call(&GetAvailableThinkingLevelsCommand {}).ok(),
				client.call(&GetSubagentsCommand {}).ok(),
			)
		});
		cx.spawn(async move |this, cx| {
			let (models, levels, subagents) = fetch.await;
			_ = this.update(cx, |this, cx| {
				this.models = models.unwrap_or_default();
				this.thinking_levels = levels.unwrap_or_default();
				if let Some(subagents) = subagents {
					this.model.load_subagents(subagents);
				}
				cx.notify();
			});
		})
		.detach();
	}

	/// Runs a control command off the UI thread with its controls disabled,
	/// then refetches the state; `label` names it in the error notice.
	fn control<T: Send + 'static>(
		&mut self,
		pending: PendingControl,
		label: &'static str,
		call: impl FnOnce(&Client) -> Result<T, omp_rpc::Error> + Send + 'static,
		cx: &mut Context<Self>,
	) {
		let Some(client) = self.client() else {
			return;
		};
		if self.model.pending.is_some() {
			return;
		}
		self.model.pending = Some(pending);
		cx.notify();
		let call = cx.background_spawn(async move { call(&client).map(|_| ()) });
		cx.spawn(async move |this, cx| {
			let result = call.await;
			_ = this.update(cx, |this, cx| {
				this.model.pending = None;
				match result {
					// `manual_compaction_end` already said why nothing was compacted.
					Err(omp_rpc::Error::Command { .. }) if pending == PendingControl::Compact => {},
					Err(error) => this.model.notices.push(Notice::Error(format!("{label}失败：{error}"))),
					Ok(()) => {},
				}
				this.refresh_state(cx);
				cx.notify();
			});
		})
		.detach();
	}

	fn set_model(&mut self, provider: String, model_id: String, cx: &mut Context<Self>) {
		self.control(PendingControl::Model, "切换模型", move |c| c.call(&SetModelCommand { provider, model_id }), cx);
	}

	fn set_thinking(&mut self, level: ThinkingLevel, cx: &mut Context<Self>) {
		self.control(PendingControl::Thinking, "切换思考档位", move |c| c.call(&SetThinkingLevelCommand { level }), cx);
	}

	fn compact(&mut self, cx: &mut Context<Self>) {
		self.control(
			PendingControl::Compact,
			"压缩",
			|c| c.call(&CompactCommand { custom_instructions: None }),
			cx,
		);
	}

	fn set_auto_compaction(&mut self, enabled: bool, cx: &mut Context<Self>) {
		self.control(
			PendingControl::AutoCompact,
			"自动压缩",
			move |c| c.call(&SetAutoCompactionCommand { enabled }),
			cx,
		);
	}

	fn cancel_subagent(&mut self, subagent_id: String, cx: &mut Context<Self>) {
		let Some(client) = self.client() else {
			return;
		};
		let call = cx.background_spawn(async move { client.call(&CancelSubagentCommand { subagent_id }) });
		cx.spawn(async move |this, cx| {
			let notice = match call.await {
				// The row updates through `subagent_lifecycle`.
				Ok(true) => return,
				Ok(false) => Notice::Info("子 agent 已经结束".into()),
				Err(error) => Notice::Error(format!("取消子 agent 失败：{error}")),
			};
			_ = this.update(cx, |this, cx| {
				this.model.notices.push(notice);
				cx.notify();
			});
		})
		.detach();
	}

	/// Opens the steer input for a subagent, or sends its text when open.
	fn steer_subagent(&mut self, subagent_id: String, window: &mut Window, cx: &mut Context<Self>) {
		let Some(input) = self.steer_inputs.get(&subagent_id).map(|(input, _)| input.clone()) else {
			let input = cx.new(|cx| InputState::new(window, cx).placeholder("给子 agent 的指令，Enter 发送"));
			let id = subagent_id.clone();
			let sub = cx.subscribe_in(&input, window, move |this, _, event, window, cx| {
				if let InputEvent::PressEnter { .. } = event {
					this.steer_subagent(id.clone(), window, cx);
				}
			});
			input.update(cx, |state, cx| state.focus(window, cx));
			self.steer_inputs.insert(subagent_id, (input, sub));
			cx.notify();
			return;
		};
		let message = input.read(cx).value().trim().to_owned();
		if message.is_empty() {
			return;
		}
		let Some(client) = self.client() else {
			return;
		};
		self.steer_inputs.remove(&subagent_id);
		cx.notify();
		let call = cx.background_spawn(async move { client.call(&SteerSubagentCommand { subagent_id, message }) });
		cx.spawn(async move |this, cx| {
			if let Err(error) = call.await {
				_ = this.update(cx, |this, cx| {
					this.model.notices.push(Notice::Error(format!("干预子 agent 失败：{error}")));
					cx.notify();
				});
			}
		})
		.detach();
	}

	fn open_panel(&mut self, cx: &mut Context<Self>) {
		if !self.panel {
			self.toggle_panel(cx);
		}
	}

	pub fn toggle_panel(&mut self, cx: &mut Context<Self>) {
		self.panel = !self.panel;
		if self.panel {
			self.refresh_state(cx);
		}
		cx.notify();
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
						.flex()
						.flex_col()
						.gap_2()
						.children(view.blocks.iter().enumerate().map(|(bi, block)| match block {
							Block::Text(text) => div().child(text.clone()).into_any_element(),
							Block::Image { mime_type, data } => self.render_image(index, bi, mime_type, data, cx),
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

	/// A transcript image, decoded once per (row, block); a label when the
	/// message carried no data or an unknown format.
	fn render_image(&self, row: usize, bi: usize, mime_type: &str, data: &str, cx: &App) -> AnyElement {
		let p = theme::palette(cx);
		let image = self
			.images
			.borrow_mut()
			.entry((row, bi))
			.or_insert_with(|| attachments::decode(mime_type, data))
			.clone();
		match image {
			Some(image) => div()
				.id(("transcript-image", row * 64 + bi))
				.child(img(image).max_w(px(320.)).max_h(px(240.)).object_fit(ObjectFit::Contain).rounded(px(6.)))
				.test_support()
				.into_any_element(),
			None => {
				div().text_xs().text_color(rgb(p.muted_foreground)).child(format!("[图片 {mime_type}]")).into_any_element()
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
			Block::Image { mime_type, data } => self.render_image(row, bi, mime_type, data, cx),
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
		let connected = matches!(self.connection, Connection::Connected(_));
		let pending = self.model.pending;
		let this = cx.entity().downgrade();

		let current = state.and_then(|s| s.model.as_ref()).map(|m| (m.provider.clone(), m.id.clone()));
		let models = self.models.clone();
		let model_menu = {
			let this = this.clone();
			let current = current.clone();
			Button::new("status-model")
				.ghost()
				.xsmall()
				.label(current.as_ref().map_or_else(|| "模型".to_owned(), |(_, id)| format!("{id} ▾")))
				.tooltip("切换模型（仅本会话）")
				.loading(pending == Some(PendingControl::Model))
				.disabled(!connected || pending.is_some() || models.is_empty())
				.dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
					let mut menu = menu.scrollable(true).max_h(px(420.));
					for model in &models {
						let key = (model.provider.clone(), model.id.clone());
						let this = this.clone();
						let checked = current.as_ref() == Some(&key);
						menu = menu.item(PopupMenuItem::new(model.id.clone()).checked(checked).on_click(move |_, _, cx| {
							let (provider, id) = key.clone();
							_ = this.update(cx, |view, cx| view.set_model(provider, id, cx));
						}));
					}
					menu
				})
		};

		let level = state.and_then(|s| s.thinking_level);
		let levels = self.thinking_levels.clone();
		let thinking_menu = {
			let this = this.clone();
			Button::new("status-thinking")
				.ghost()
				.xsmall()
				.label(format!("思考 {} ▾", level.map_or("—", |l| l.as_str())))
				.tooltip("切换思考档位（仅本会话）")
				.loading(pending == Some(PendingControl::Thinking))
				.disabled(!connected || pending.is_some() || levels.is_empty())
				.dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
					for &option in &levels {
						let this = this.clone();
						menu = menu.item(PopupMenuItem::new(option.as_str()).checked(level == Some(option)).on_click(
							move |_, _, cx| {
								_ = this.update(cx, |view, cx| view.set_thinking(option, cx));
							},
						));
					}
					menu
				})
		};

		let usage = state.and_then(|s| s.context_usage.as_ref());
		let context_color = |percent: f64| match percent {
			p_ if p_ >= status::CONTEXT_DANGER => p.danger,
			p_ if p_ >= status::CONTEXT_WARN => p.warning,
			_ => p.primary,
		};
		let context = usage.map(|u| {
			div()
				.flex()
				.items_center()
				.gap_1()
				.child(Progress::new("status-context-bar").w(px(48.)).value(u.percent as f32).color(rgb(context_color(u.percent))))
				.child(
					div().text_color(rgb(if u.percent >= status::CONTEXT_WARN { context_color(u.percent) } else { p.muted_foreground })).child(
						format!("{:.0}% ({} / {})", u.percent, status::tokens(u.tokens), status::tokens(u.context_window)),
					),
				)
		});
		let compacting = self.model.compacting || state.is_some_and(|s| s.is_compacting);
		let compact = Button::new("status-compact")
			.ghost()
			.xsmall()
			.label(if compacting { "压缩中…" } else { "压缩" })
			.tooltip("压缩上下文（compact）")
			.loading(compacting || pending == Some(PendingControl::Compact))
			.disabled(!connected || pending.is_some() || compacting || self.model.busy())
			.on_click(cx.listener(|this, _, _, cx| this.compact(cx)));

		let cost = self.stats.as_ref().map(|s| format!("${:.3}", s.cost));
		let (done, total) = state.map_or((0, 0), |s| status::todo_counts(&s.todo_phases));
		let running = self.model.running_subagents();
		let subagents = self.model.subagents.len();
		div()
			.h(px(26.))
			.flex_none()
			.flex()
			.items_center()
			.gap_3()
			.px_3()
			.border_t_1()
			.border_color(rgb(p.border))
			.bg(rgb(p.panel))
			.text_xs()
			.text_color(rgb(p.muted_foreground))
			.child(div().flex().items_center().gap_1().child(div().size(px(7.)).rounded_full().bg(rgb(dot))).child(label))
			.child(div().min_w_0().truncate().child(self.cwd.display().to_string()))
			.child(div().flex_1())
			.when_some(self.model.status.clone(), |row, status| row.child(status))
			.child(model_menu)
			.child(thinking_menu)
			.children(context)
			.child(compact)
			.children(cost)
			.when(total > 0, |row| {
				row.child(
					Button::new("status-todo")
						.ghost()
						.xsmall()
						.label(format!("☑ Todo {done}/{total}"))
						.on_click(cx.listener(|this, _, _, cx| this.open_panel(cx))),
				)
			})
			.when(subagents > 0, |row| {
				row.child(
					Button::new("status-subagents")
						.ghost()
						.xsmall()
						.label(if running > 0 { format!("⑂ 子 agent {running} 运行中") } else { format!("⑂ 子 agent {subagents}") })
						.on_click(cx.listener(|this, _, _, cx| this.open_panel(cx))),
				)
			})
			.child(
				Button::new("status-panel")
					.ghost()
					.xsmall()
					.selected(self.panel)
					.label("详情")
					.tooltip("上下文、费用、Todo、子 agent 与会话状态（⌘J）")
					.on_click(cx.listener(|this, _, _, cx| this.toggle_panel(cx))),
			)
	}

	fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let state = self.model.state.as_ref();
		let connected = matches!(self.connection, Connection::Connected(_));
		let pending = self.model.pending;
		let heading = |text: &'static str| {
			div().mt_3().mb_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(p.muted_foreground)).child(text)
		};
		let kv = |key: String, value: String| {
			div()
				.flex()
				.gap_2()
				.text_xs()
				.child(div().w(px(92.)).flex_none().text_color(rgb(p.muted_foreground)).child(key))
				.child(div().flex_1().min_w_0().font_family("Menlo").child(value))
		};

		// Context and compaction.
		let usage = state.and_then(|s| s.context_usage.as_ref());
		let auto = state.is_some_and(|s| s.auto_compaction_enabled);
		let context = div()
			.flex()
			.flex_col()
			.gap_1()
			.child(heading("上下文"))
			.child(
				Progress::new("panel-context-bar").value(usage.map_or(0., |u| u.percent as f32)).color(rgb(match usage {
					Some(u) if u.percent >= status::CONTEXT_DANGER => p.danger,
					Some(u) if u.percent >= status::CONTEXT_WARN => p.warning,
					_ => p.primary,
				})),
			)
			.child(kv(
				"已用".into(),
				usage.map_or("—".into(), |u| {
					format!("{} / {}（{:.1}%）", status::tokens(u.tokens), status::tokens(u.context_window), u.percent)
				}),
			))
			.child(
				div().flex().items_center().justify_between().child(
					Switch::new("panel-auto-compact")
						.xsmall()
						.label("自动压缩")
						.checked(auto)
						.disabled(!connected || pending.is_some())
						.on_click(cx.listener(|this, checked: &bool, _, cx| this.set_auto_compaction(*checked, cx))),
				),
			);

		// Cost.
		let cost = div().flex().flex_col().gap_1().child(heading("费用")).children(self.stats.as_ref().map(|s| {
			div()
				.flex()
				.flex_col()
				.gap_1()
				.child(kv("累计".into(), format!("${:.4}", s.cost)))
				.child(kv("输入".into(), status::tokens(s.tokens.input)))
				.child(kv("输出".into(), status::tokens(s.tokens.output)))
				.child(kv("缓存读/写".into(), format!("{} / {}", status::tokens(s.tokens.cache_read), status::tokens(s.tokens.cache_write))))
				.child(kv("消息 / 工具".into(), format!("{} / {}", s.total_messages, s.tool_calls)))
		}));

		// Todos.
		let phases = state.map(|s| s.todo_phases.as_slice()).unwrap_or_default();
		let todo = div().flex().flex_col().gap_0p5().child(heading("Todo")).when(phases.is_empty(), |col| {
			col.child(div().text_xs().text_color(rgb(p.muted_foreground)).child("暂无"))
		});
		let todo = phases.iter().enumerate().fold(todo, |col, (pi, phase)| {
			let header = div().id(("todo-phase", pi)).text_xs().font_weight(FontWeight::MEDIUM).child(phase.name.clone());
			phase.tasks.iter().fold(col.child(header), |col, task| {
				let done = matches!(task.status, omp_rpc::TodoStatus::Completed | omp_rpc::TodoStatus::Abandoned);
				col.child(
					div()
						.flex()
						.gap_1()
						.pl_2()
						.text_xs()
						.when(done, |row| row.text_color(rgb(p.muted_foreground)))
						.child(div().flex_none().child(status::todo_mark(task.status)))
						.child(div().min_w_0().child(task.content.clone()))
						.when_some(task.blocker.clone(), |row, blocker| {
							row.child(div().text_color(rgb(p.warning)).child(format!("（{blocker}）")))
						}),
				)
			})
		});

		// Subagents.
		let subagents = div().flex().flex_col().gap_2().child(heading("子 agent")).when(self.model.subagents.is_empty(), |col| {
			col.child(div().text_xs().text_color(rgb(p.muted_foreground)).child("暂无"))
		});
		let subagents = self.model.subagents.iter().enumerate().fold(subagents, |col, (row, agent)| {
			let color = match agent.status {
				SubagentStatus::Pending | SubagentStatus::Running => p.primary,
				SubagentStatus::Completed => p.success,
				SubagentStatus::Failed => p.danger,
				SubagentStatus::Aborted => p.warning,
			};
			let progress = agent.progress.as_ref();
			let field = |key: &str| progress.and_then(|p| p.get(key));
			let activity = field("currentTool")
				.and_then(|v| v.as_str())
				.map(|tool| format!("正在运行 {tool}"))
				.or_else(|| field("lastIntent").and_then(|v| v.as_str()).map(str::to_owned));
			let mut metrics = Vec::new();
			if let Some(n) = field("toolCount").and_then(|v| v.as_i64()) {
				metrics.push(format!("{n} 次工具"));
			}
			if let Some(n) = field("tokens").and_then(|v| v.as_i64()).filter(|n| *n > 0) {
				metrics.push(format!("{} tok", status::tokens(n)));
			}
			if let Some(ms) = field("durationMs").and_then(|v| v.as_f64()).filter(|ms| *ms > 0.) {
				metrics.push(format!("{:.0}s", ms / 1000.));
			}
			if let Some(cost) = field("cost").and_then(|v| v.as_f64()).filter(|c| *c > 0.) {
				metrics.push(format!("${cost:.3}"));
			}
			let steer = agent.id.as_ref().and_then(|id| self.steer_inputs.get(id)).map(|(input, _)| input.clone());
			let card = div()
				.id(("subagent", row))
				.flex()
				.flex_col()
				.gap_1()
				.p_2()
				.rounded(px(6.))
				.border_1()
				.border_color(rgb(p.border))
				.bg(rgb(p.elevated))
				.text_xs()
				.child(
					div()
						.flex()
						.items_center()
						.gap_1()
						.child(div().size(px(7.)).rounded_full().bg(rgb(color)))
						.child(div().font_weight(FontWeight::SEMIBOLD).child(agent.agent.clone()))
						.child(div().text_color(rgb(p.muted_foreground)).child(agent.status.as_str())),
				)
				.when_some(agent.task.clone(), |card, task| card.child(div().child(truncate(&task, 120))))
				.when_some(activity, |card, text| {
					card.child(div().text_color(rgb(p.muted_foreground)).font_family("Menlo").child(truncate(&text, 80)))
				})
				.when(!metrics.is_empty(), |card| {
					card.child(div().text_color(rgb(p.muted_foreground)).child(metrics.join(" · ")))
				})
				.when_some(steer, |card, input| {
					card.child(div().id(("subagent-steer-input", row)).child(Input::new(&input).small()).test_support())
				});
			let card = match (&agent.id, agent.running()) {
				(Some(id), true) => {
					let (steer_id, cancel_id) = (id.clone(), id.clone());
					card.child(
						div()
							.flex()
							.gap_1()
							.child(
								Button::new(("subagent-steer", row))
									.xsmall()
									.label("干预")
									.tooltip("给子 agent 发一条指令")
									.disabled(!connected)
									.on_click(cx.listener(move |this, _, window, cx| {
										this.steer_subagent(steer_id.clone(), window, cx)
									})),
							)
							.child(
								Button::new(("subagent-cancel", row))
									.xsmall()
									.danger()
									.label("取消")
									.disabled(!connected)
									.on_click(cx.listener(move |this, _, _, cx| this.cancel_subagent(cancel_id.clone(), cx))),
							),
					)
				},
				_ => card,
			};
			col.child(card)
		});

		// Every `get_state` field.
		let fields = div().flex().flex_col().gap_0p5().child(heading("会话状态")).children(
			state.map(status::state_rows).unwrap_or_default().into_iter().map(|(key, label, value)| {
				div().id(SharedString::from(format!("state-{key}"))).child(kv(label.into(), value)).test_support()
			}),
		);

		div()
			.id("detail-panel")
			.w(px(300.))
			.flex_none()
			.h_full()
			.overflow_y_scroll()
			.border_l_1()
			.border_color(rgb(p.border))
			.bg(rgb(p.panel))
			.px_3()
			.pb_4()
			.child(context)
			.child(cost)
			.child(todo)
			.child(subagents)
			.child(fields)
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
			.when(!self.attachments.is_empty(), |col| col.child(self.render_attachments(cx)))
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
							// Images on the clipboard attach; text pastes as usual.
							.capture_action(cx.listener(|this, _: &Paste, _, cx| {
								if let Some(item) = cx.read_from_clipboard()
									&& this.attach(attachments::from_clipboard(&item), cx)
								{
									cx.stop_propagation();
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
			.child(
				Button::new("reconnect")
					.primary()
					.label("重新连接")
					.on_click(cx.listener(|this, _, _, cx| this.reconnect(cx))),
			)
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
			.id("chat-root")
			.size_full()
			.relative()
			.flex()
			.flex_col()
			.bg(rgb(p.background))
			// Image files dropped anywhere on the tab attach to the composer.
			.drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(rgb(p.accent)))
			.on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
				this.attach(attachments::from_paths(paths.paths()), cx);
			}))
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
						.child(
							Button::new("reconnect-banner")
								.small()
								.label("重新连接")
								.on_click(cx.listener(|this, _, _, cx| this.reconnect(cx))),
						),
				)
			})
			.child(
				div()
					.flex_1()
					.min_h_0()
					.flex()
					.child(div().flex_1().min_w_0().flex().flex_col().child(body).child(self.render_composer(cx)))
					.when(self.panel, |row| row.child(self.render_panel(cx))),
			)
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
