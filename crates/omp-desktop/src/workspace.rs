//! The window: session tabs in the title bar (one omp process each) and a
//! sidebar listing saved sessions grouped by working directory.
//!
//! Shortcuts: ⌘N new tab · ⌘W close tab · ⌘1–9 switch tab · ⌘P search
//! sessions · ⌘B toggle the sidebar · ⌘J status panel · ⌘K command palette
//! · ⌘M pick a model · ⌘T cycle the thinking level · ⌘⇧L accounts.

use std::{
	collections::HashSet,
	path::{Path, PathBuf},
	time::{Duration, SystemTime},
};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;
use omp_bridge::SessionOptions;
use omp_store::sessions::{self, Scan, SessionInfo};

use crate::accounts::{AccountsEvent, AccountsView};
use crate::chat::{ChatEvent, ChatView, TabStatus, TabSummary};
use crate::palette::{Command, Entry, Group};
use crate::palette_view::{PaletteEvent, PaletteView};
use crate::theme::{self, ThemeChoice};

/// The session directory has no change notification we rely on; a scan reads
/// at most 4 KiB per file, so polling is cheap. Tabs also trigger a rescan
/// when their session file appears or a turn settles.
const RESCAN_EVERY: Duration = Duration::from_secs(3);

pub struct Workspace {
	/// Launch settings shared by every tab (`omp` path, `--session-dir`,
	/// extra flags); `cwd` and `resume` are set per tab.
	base:        SessionOptions,
	default_cwd: PathBuf,
	/// Where saved sessions are listed from.
	root:        Option<PathBuf>,
	tabs:        Vec<Tab>,
	active:      usize,
	scan:        Scan,
	search:      Entity<InputState>,
	sidebar:     bool,
	/// Sidebar groups the user folded, by cwd.
	collapsed:   HashSet<PathBuf>,
	/// The accounts modal, while open; it runs its own omp process.
	accounts:    Option<(Entity<AccountsView>, Subscription)>,
	/// The command palette (⌘K, or ⌘M for models only), while open.
	palette:     Option<(Entity<PaletteView>, Subscription)>,
	_tasks:      Vec<Task<()>>,
	_subs:       Vec<Subscription>,
}

struct Tab {
	view:    Entity<ChatView>,
	/// Last summary seen, so streaming updates that change nothing visible in
	/// the tab bar do not re-render the workspace.
	summary: TabSummary,
	_subs:   [Subscription; 2],
}

impl Workspace {
	pub fn new(base: SessionOptions, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let root = base.session_dir.clone().or_else(sessions::default_root);
		let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索会话（标题 / 路径）  ⌘P"));
		let sub = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
			if matches!(event, InputEvent::Change) {
				cx.notify();
			}
		});
		let mut this = Self {
			base: SessionOptions { cwd: None, resume: None, ..base },
			default_cwd: cwd.clone(),
			root,
			tabs: Vec::new(),
			active: 0,
			scan: Scan::default(),
			search,
			sidebar: true,
			collapsed: HashSet::new(),
			accounts: None,
			palette: None,
			_tasks: Vec::new(),
			_subs: vec![sub],
		};
		// "Follow the system" tracks appearance changes while running.
		let appearance = cx.observe_window_appearance(window, |_, window, cx| {
			theme::appearance_changed(window, cx);
		});
		this._subs.push(appearance);
		this.new_tab(cwd, window, cx);
		let poll = cx.spawn(async move |this, cx| {
			loop {
				if this.update(cx, |this, cx| this.rescan(cx)).is_err() {
					return;
				}
				cx.background_executor().timer(RESCAN_EVERY).await;
			}
		});
		this._tasks.push(poll);
		this
	}

	/// Re-reads the session list off the UI thread.
	pub fn rescan(&mut self, cx: &mut Context<Self>) {
		let Some(root) = self.root.clone() else {
			return;
		};
		let scan = cx.background_spawn(async move { sessions::scan(&root) });
		cx.spawn(async move |this, cx| {
			let scan = scan.await;
			_ = this.update(cx, |this, cx| {
				if this.scan != scan {
					this.scan = scan;
					cx.notify();
				}
			});
		})
		.detach();
	}

	fn push_tab(&mut self, view: Entity<ChatView>, window: &mut Window, cx: &mut Context<Self>) {
		let summary = view.read(cx).summary();
		let observe = cx.observe(&view, |this, view, cx| this.tab_changed(&view, cx));
		let events = cx.subscribe_in(&view, window, |this, _, event: &ChatEvent, window, cx| match event {
			ChatEvent::OpenAccounts => this.open_accounts(window, cx),
		});
		self.tabs.push(Tab { view, summary, _subs: [observe, events] });
		self.active = self.tabs.len() - 1;
		cx.notify();
	}

	pub fn new_tab(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
		let options = SessionOptions { cwd: Some(cwd), ..self.base.clone() };
		let view = cx.new(|cx| ChatView::new(options, window, cx));
		self.push_tab(view, window, cx);
	}

	/// Opens a saved session: switches to its tab when one already has it,
	/// otherwise resumes it in a new tab. `fork` continues a copy instead.
	pub fn open_session(&mut self, path: &Path, cwd: &Path, fork: bool, window: &mut Window, cx: &mut Context<Self>) {
		if !fork && let Some(index) = self.tab_with_file(path) {
			self.activate(index, window, cx);
			return;
		}
		let options = SessionOptions { cwd: Some(cwd.to_owned()), resume: Some(path.to_owned()), ..self.base.clone() };
		let view = cx.new(|cx| {
			if fork { ChatView::fork(options, window, cx) } else { ChatView::new(options, window, cx) }
		});
		self.push_tab(view, window, cx);
	}

	fn tab_with_file(&self, path: &Path) -> Option<usize> {
		self.tabs.iter().position(|tab| tab.summary.session_file.as_deref() == Some(path))
	}

	pub fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
		let Some(tab) = self.tabs.get(index) else {
			return;
		};
		self.active = index;
		let view = tab.view.clone();
		view.update(cx, |view, cx| view.focus_composer(window, cx));
		cx.notify();
	}

	/// Closes a tab and stops its omp process. The window always keeps one
	/// tab: closing the last opens a new session in the same directory.
	pub fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
		if index >= self.tabs.len() {
			return;
		}
		let tab = self.tabs.remove(index);
		tab.view.update(cx, |view, cx| view.shutdown(cx));
		if self.tabs.is_empty() {
			self.new_tab(tab.summary.cwd, window, cx);
			return;
		}
		if index < self.active || self.active >= self.tabs.len() {
			self.active = self.active.saturating_sub(1);
		}
		self.activate(self.active, window, cx);
	}

	fn tab_changed(&mut self, view: &Entity<ChatView>, cx: &mut Context<Self>) {
		let summary = view.read(cx).summary();
		let Some(tab) = self.tabs.iter_mut().find(|tab| tab.view.entity_id() == view.entity_id()) else {
			return;
		};
		if tab.summary == summary {
			return;
		}
		let file_changed = tab.summary.session_file != summary.session_file;
		let settled = summary.status == TabStatus::Idle && tab.summary.status != TabStatus::Idle;
		tab.summary = summary;
		if file_changed || settled {
			self.rescan(cx);
		}
		cx.notify();
	}

	pub fn open_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		if self.accounts.is_some() {
			return;
		}
		let view = cx.new(|cx| AccountsView::new(&self.base, window, cx));
		let sub = cx.subscribe_in(&view, window, |this, _, event: &AccountsEvent, window, cx| match event {
			AccountsEvent::Close => this.close_accounts(window, cx),
			// Tabs that could not start for lack of credentials can now.
			AccountsEvent::Changed => {
				for tab in &this.tabs {
					tab.view.update(cx, |view, cx| view.reconnect_if_failed(cx));
				}
			},
		});
		self.accounts = Some((view, sub));
		cx.notify();
	}

	fn close_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		// Dropping the view stops its omp process.
		if self.accounts.take().is_some()
			&& let Some(view) = self.tabs.get(self.active).map(|tab| tab.view.clone())
		{
			view.update(cx, |view, cx| view.focus_composer(window, cx));
		}
		cx.notify();
	}

	/// ⌘K: window actions plus the active tab's models, levels, commands.
	/// ⌘M: the same palette narrowed to models.
	pub fn open_palette(&mut self, models_only: bool, window: &mut Window, cx: &mut Context<Self>) {
		if self.palette.is_some() || self.accounts.is_some() {
			return;
		}
		let tab = self.tabs.get(self.active).map(|tab| tab.view.read(cx).palette_entries()).unwrap_or_default();
		let entries: Vec<Entry> = if models_only {
			tab.into_iter().filter(|e| e.group == Group::Model).collect()
		} else {
			let theme = theme::choice(cx);
			let mut entries = vec![
				Entry::new(Group::Action, "新建会话", Command::NewTab).detail("new tab").shortcut("⌘N"),
				Entry::new(Group::Action, "关闭会话", Command::CloseTab).detail("close tab").shortcut("⌘W"),
				Entry::new(Group::Action, "搜索会话", Command::SearchSessions).detail("search sessions").shortcut("⌘P"),
				Entry::new(Group::Action, "显示 / 隐藏会话栏", Command::ToggleSidebar).detail("sidebar").shortcut("⌘B"),
				Entry::new(Group::Action, "状态面板", Command::TogglePanel).detail("status panel").shortcut("⌘J"),
				Entry::new(Group::Action, "循环思考档位", Command::CycleThinking).detail("cycle thinking").shortcut("⌘T"),
				Entry::new(Group::Action, "压缩上下文", Command::Compact).detail("compact"),
				Entry::new(Group::Action, "账号与登录", Command::Accounts).detail("accounts login logout").shortcut("⌘⇧L"),
			];
			entries.extend(ThemeChoice::ALL.map(|choice| {
				Entry::new(Group::Action, format!("主题：{}", choice.label()), Command::Theme(choice))
					.detail("theme appearance")
					.current(choice == theme)
			}));
			entries.extend(tab);
			entries
		};
		let placeholder = if models_only { "切换模型（仅本会话）" } else { "输入命令、模型或 /命令" };
		let view = cx.new(|cx| PaletteView::new(entries, placeholder, window, cx));
		let sub = cx.subscribe_in(&view, window, |this, _, event: &PaletteEvent, window, cx| match event {
			PaletteEvent::Close => this.close_palette(window, cx),
			PaletteEvent::Run(command) => {
				let command = command.clone();
				this.close_palette(window, cx);
				this.run_command(command, window, cx);
			},
		});
		self.palette = Some((view, sub));
		cx.notify();
	}

	fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		if self.palette.take().is_some()
			&& let Some(view) = self.tabs.get(self.active).map(|tab| tab.view.clone())
		{
			view.update(cx, |view, cx| view.focus_composer(window, cx));
		}
		cx.notify();
	}

	fn run_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
		match command {
			Command::NewTab => {
				let cwd = self.tabs.get(self.active).map_or(self.default_cwd.clone(), |tab| tab.summary.cwd.clone());
				self.new_tab(cwd, window, cx);
			},
			Command::CloseTab => self.close_tab(self.active, window, cx),
			Command::SearchSessions => self.focus_search(window, cx),
			Command::ToggleSidebar => {
				self.sidebar = !self.sidebar;
				cx.notify();
			},
			Command::Accounts => self.open_accounts(window, cx),
			Command::Theme(choice) => theme::set_choice(choice, window, cx),
			command => {
				if let Some(view) = self.tabs.get(self.active).map(|tab| tab.view.clone()) {
					view.update(cx, |view, cx| view.run_palette(command, window, cx));
				}
			},
		}
	}

	#[cfg(test)]
	pub(crate) fn palette_view(&self) -> Option<Entity<PaletteView>> {
		self.palette.as_ref().map(|(view, _)| view.clone())
	}

	#[cfg(test)]
	pub(crate) fn accounts_view(&self) -> Option<Entity<AccountsView>> {
		self.accounts.as_ref().map(|(view, _)| view.clone())
	}

	fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		self.sidebar = true;
		self.search.update(cx, |state, cx| state.focus(window, cx));
		self.rescan(cx);
		cx.notify();
	}

	fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
		let keystroke = &event.keystroke;
		let modifiers = &keystroke.modifiers;
		if keystroke.key == "escape" && self.accounts.is_some() {
			self.close_accounts(window, cx);
			cx.stop_propagation();
			return;
		}
		// The palette's input handles its own keys (Esc included).
		if self.palette.is_some() {
			return;
		}
		if !modifiers.secondary(cx) || modifiers.alt {
			return;
		}
		if modifiers.shift {
			if keystroke.key == "l" {
				self.open_accounts(window, cx);
				cx.stop_propagation();
			}
			return;
		}
		match keystroke.key.as_str() {
			"n" => {
				let cwd = self.tabs.get(self.active).map_or(self.default_cwd.clone(), |tab| tab.summary.cwd.clone());
				self.new_tab(cwd, window, cx);
			},
			"w" => self.close_tab(self.active, window, cx),
			"p" => self.focus_search(window, cx),
			"b" => {
				self.sidebar = !self.sidebar;
				cx.notify();
			},
			"j" => self.run_command(Command::TogglePanel, window, cx),
			"t" => self.run_command(Command::CycleThinking, window, cx),
			"k" => self.open_palette(false, window, cx),
			"m" => self.open_palette(true, window, cx),
			key => match key.parse::<usize>() {
				Ok(n @ 1..=9) => self.activate(n - 1, window, cx),
				_ => return,
			},
		}
		cx.stop_propagation();
	}

	fn title_of(&self, summary: &TabSummary) -> String {
		if let Some(name) = &summary.name {
			return name.clone();
		}
		if let Some(file) = &summary.session_file
			&& let Some(title) = self.scan.sessions.iter().find(|s| &s.path == file).and_then(|s| s.title.clone())
		{
			return title;
		}
		summary.cwd.file_name().map_or_else(|| summary.cwd.display().to_string(), |n| n.to_string_lossy().into_owned())
	}

	#[cfg(test)]
	pub(crate) fn tab_views(&self) -> Vec<Entity<ChatView>> {
		self.tabs.iter().map(|tab| tab.view.clone()).collect()
	}

	#[cfg(test)]
	pub(crate) fn active_index(&self) -> usize {
		self.active
	}

	#[cfg(test)]
	pub(crate) fn listed(&self) -> &[SessionInfo] {
		&self.scan.sessions
	}

	fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
			let active = index == self.active;
			let (dot, badge) = match tab.summary.status {
				TabStatus::Connecting => (p.warning, None),
				TabStatus::Idle => (p.success, None),
				TabStatus::Running => (p.primary, Some("•••")),
				TabStatus::Attention => (p.warning, Some("待确认")),
				TabStatus::Disconnected => (p.danger, Some("已断开")),
			};
			div()
				.id(("tab", index))
				.flex()
				.items_center()
				.gap_1()
				.h(px(26.))
				.pl_2()
				.max_w(px(220.))
				.rounded(px(6.))
				.text_sm()
				.cursor_pointer()
				.when(active, |tab| tab.bg(rgb(p.elevated)).border_1().border_color(rgb(p.border)))
				.when(!active, |tab| tab.text_color(rgb(p.muted_foreground)).hover(|s| s.bg(rgb(p.muted))))
				.child(div().flex_none().size(px(7.)).rounded_full().bg(rgb(dot)))
				.child(div().min_w_0().truncate().child(self.title_of(&tab.summary)))
				.when_some(badge, |row, badge| {
					row.child(div().flex_none().text_xs().text_color(rgb(dot)).child(badge))
				})
				.child(
					Button::new(("tab-close", index))
						.ghost()
						.xsmall()
						.label("×")
						.tooltip("关闭（⌘W），结束该 omp 进程；bash 放到后台的任务不受影响")
						.on_click(cx.listener(move |this, _, window, cx| {
							cx.stop_propagation();
							this.close_tab(index, window, cx);
						})),
				)
				.on_click(cx.listener(move |this, _, window, cx| this.activate(index, window, cx)))
				.test_support()
		});
		TitleBar::new()
			.child(
				div()
					.flex_1()
					.min_w_0()
					.h_full()
					.flex()
					.items_center()
					.gap_1()
					.children(tabs)
					.child(Button::new("tab-new").ghost().xsmall().label("+").tooltip("新建会话（⌘N）").on_click(
						cx.listener(|this, _, window, cx| {
							let cwd = this.tabs.get(this.active).map_or(this.default_cwd.clone(), |t| t.summary.cwd.clone());
							this.new_tab(cwd, window, cx);
						}),
					)),
			)
			.child(
				div().pr_2().flex().gap_1().child({
					let current = theme::choice(cx);
					Button::new("theme-menu")
						.ghost()
						.xsmall()
						.label("主题")
						.tooltip(format!("外观：{}", current.label()))
						.dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
							for choice in ThemeChoice::ALL {
								menu = menu.item(PopupMenuItem::new(choice.label()).checked(choice == current).on_click(
									move |_, window, cx| theme::set_choice(choice, window, cx),
								));
							}
							menu
						})
				}).child(
					Button::new("accounts-open")
						.ghost()
						.xsmall()
						.label("账号")
						.tooltip("登录 / 退出 provider（⌘⇧L）")
						.on_click(cx.listener(|this, _, window, cx| this.open_accounts(window, cx))),
				).child(
					Button::new("sidebar-toggle")
						.ghost()
						.xsmall()
						.label(if self.sidebar { "隐藏会话栏" } else { "会话栏" })
						.tooltip("⌘B")
						.on_click(cx.listener(|this, _, _, cx| {
							this.sidebar = !this.sidebar;
							cx.notify();
						})),
				),
			)
	}

	fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let query = self.search.read(cx).value().to_string();
		let searching = !query.trim().is_empty();
		let groups = sessions::group(&self.scan.sessions, &query);
		let selected = self.tabs.get(self.active).and_then(|tab| tab.summary.session_file.clone());
		let open: HashSet<PathBuf> = self.tabs.iter().filter_map(|tab| tab.summary.session_file.clone()).collect();
		let now = SystemTime::now();
		let mut row = 0usize;
		let mut items: Vec<AnyElement> = Vec::new();
		for (gi, group) in groups.iter().enumerate() {
			let cwd = group.cwd.to_owned();
			let folded = !searching && self.collapsed.contains(&cwd);
			let (toggle, new_cwd) = (cwd.clone(), cwd.clone());
			items.push(
				div()
					.id(("group", gi))
					.flex()
					.items_center()
					.gap_1()
					.mt_2()
					.px_2()
					.h(px(24.))
					.rounded(px(6.))
					.text_xs()
					.text_color(rgb(p.muted_foreground))
					.cursor_pointer()
					.hover(|s| s.bg(rgb(p.muted)))
					.child(if folded { "▸" } else { "▾" })
					.child(div().flex_1().min_w_0().truncate().child(display_path(&cwd)))
					.child(group.sessions.len().to_string())
					.child(Button::new(("group-new", gi)).ghost().xsmall().label("+").tooltip("在此目录新建会话").on_click(
						cx.listener(move |this, _, window, cx| {
							cx.stop_propagation();
							this.new_tab(new_cwd.clone(), window, cx);
						}),
					))
					.on_click(cx.listener(move |this, _, _, cx| {
						if !this.collapsed.remove(&toggle) {
							this.collapsed.insert(toggle.clone());
						}
						cx.notify();
					}))
					.test_support()
					.into_any_element(),
			);
			if folded {
				continue;
			}
			for session in &group.sessions {
				items.push(self.render_session(row, session, selected.as_deref(), &open, now, p, cx));
				row += 1;
			}
		}
		let empty = items.is_empty();
		div()
			.id("sidebar")
			.w(px(256.))
			.flex_none()
			.h_full()
			.flex()
			.flex_col()
			.border_r_1()
			.border_color(rgb(p.border))
			.bg(rgb(p.panel))
			.child(
				div()
					.p_2()
					.flex()
					.items_center()
					.gap_1()
					.child(div().id("session-search").flex_1().child(Input::new(&self.search).small().cleanable(true)).test_support())
					.child(Button::new("sidebar-new").ghost().small().label("+").tooltip("新建会话（⌘N）").on_click(cx.listener(
						|this, _, window, cx| {
							let cwd = this.tabs.get(this.active).map_or(this.default_cwd.clone(), |t| t.summary.cwd.clone());
							this.new_tab(cwd, window, cx);
						},
					))),
			)
			.child(
				div()
					.id("session-list")
					.flex_1()
					.min_h_0()
					.overflow_y_scroll()
					.px_1()
					.pb_2()
					.children(items)
					.when(empty, |list| {
						list.child(div().p_3().text_xs().text_color(rgb(p.muted_foreground)).child(if searching {
							"没有匹配的会话"
						} else {
							"还没有保存的会话"
						}))
					}),
			)
			.when(self.scan.skipped > 0, |bar| {
				bar.child(
					div()
						.px_3()
						.py_1()
						.text_xs()
						.text_color(rgb(p.muted_foreground))
						.child(format!("已跳过 {} 个无法解析的会话文件", self.scan.skipped)),
				)
			})
	}

	#[allow(clippy::too_many_arguments)]
	fn render_session(
		&self,
		row: usize,
		session: &SessionInfo,
		selected: Option<&Path>,
		open: &HashSet<PathBuf>,
		now: SystemTime,
		p: &theme::Palette,
		cx: &mut Context<Self>,
	) -> AnyElement {
		let is_selected = selected == Some(session.path.as_path());
		let is_open = open.contains(&session.path);
		let (path, cwd) = (session.path.clone(), session.cwd.clone());
		let (fork_path, fork_cwd, reveal) = (path.clone(), cwd.clone(), path.clone());
		let title = session.title.clone().unwrap_or_else(|| "（未命名）".into());
		div()
			.id(("session-row", row))
			.flex()
			.flex_col()
			.gap_0p5()
			.px_2()
			.py_1p5()
			.mt_0p5()
			.rounded(px(6.))
			.cursor_pointer()
			.when(is_selected, |r| r.bg(rgb(p.accent)).border_1().border_color(rgb(p.primary)))
			.when(!is_selected, |r| r.hover(|s| s.bg(rgb(p.muted))))
			.child(
				div()
					.flex()
					.items_center()
					.gap_2()
					.text_sm()
					.child(
						div()
							.flex_1()
							.min_w_0()
							.truncate()
							.when(is_selected, |t| t.font_weight(FontWeight::SEMIBOLD))
							.child(title),
					)
					.child(
						div()
							.flex_none()
							.text_xs()
							.text_color(rgb(p.muted_foreground))
							.child(sessions::relative_age(session.modified, now)),
					),
			)
			.child(
				div()
					.flex()
					.items_center()
					.gap_1()
					.text_xs()
					.text_color(rgb(p.muted_foreground))
					.child(format!("#{}", session.short_id()))
					.when(is_open, |r| r.child(div().text_color(rgb(p.accent_foreground)).child("已打开")))
					.child(div().flex_1())
					.child(Button::new(("session-fork", row)).ghost().xsmall().label("分支").tooltip("复制为新会话继续（fork），原会话不变").on_click(
						cx.listener(move |this, _, window, cx| {
							cx.stop_propagation();
							this.open_session(&fork_path, &fork_cwd, true, window, cx);
						}),
					))
					.child(Button::new(("session-reveal", row)).ghost().xsmall().label("定位").tooltip("在 Finder 中显示 JSONL").on_click(
						cx.listener(move |_, _, _, cx| {
							cx.stop_propagation();
							cx.reveal_path(&reveal);
						}),
					)),
			)
			.on_click(cx.listener(move |this, _, window, cx| this.open_session(&path, &cwd, false, window, cx)))
			.test_support()
			.into_any_element()
	}
}

impl Render for Workspace {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let active = self.tabs.get(self.active).map(|tab| tab.view.clone());
		div()
			.id("workspace")
			.size_full()
			.flex()
			.flex_col()
			.bg(rgb(p.background))
			.text_color(cx.theme().foreground)
			.capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_key(event, window, cx)))
			.child(self.render_tab_bar(cx))
			.child(
				div()
					.flex_1()
					.min_h_0()
					.flex()
					.when(self.sidebar, |row| row.child(self.render_sidebar(cx)))
					.child(div().flex_1().min_w_0().h_full().relative().children(active)),
			)
			.when_some(self.palette.as_ref().map(|(view, _)| view.clone()), |root, view| {
				root.child(
					div()
						.id("palette-overlay")
						.absolute()
						.inset_0()
						.flex()
						.flex_col()
						.items_center()
						.pt(px(96.))
						.bg(hsla(0., 0., 0., 0.2))
						.on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_palette(window, cx)))
						.child(view),
				)
			})
			.when_some(self.accounts.as_ref().map(|(view, _)| view.clone()), |root, view| {
				root.child(
					div()
						.id("accounts-overlay")
						.absolute()
						.inset_0()
						.flex()
						.items_center()
						.justify_center()
						.bg(hsla(0., 0., 0., 0.35))
						.child(view),
				)
			})
	}
}

/// `~/…` for paths under the home directory.
fn display_path(path: &Path) -> String {
	if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
		&& let Ok(rest) = path.strip_prefix(&home)
	{
		return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
	}
	path.display().to_string()
}
