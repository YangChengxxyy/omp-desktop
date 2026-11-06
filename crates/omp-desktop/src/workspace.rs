//! The window: session tabs in the title bar (one omp process each) and a
//! sidebar listing saved sessions grouped by working directory.
//!
//! Shortcuts: ⌘N new tab · ⌘W close tab · ⌘1–9 switch tab · ⌘P search
//! sessions · ⌘B toggle the sidebar · ⌘J status panel · ⌘K command palette
//! · ⌘M pick a model · ⌘T cycle the thinking level · ⌘⇧L accounts · ⌘, settings.

use std::{
	collections::{HashMap, HashSet},
	path::{Path, PathBuf},
	time::{Duration, SystemTime},
};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;
use omp_bridge::SessionOptions;
use omp_store::sessions::{self, Scan, SessionInfo};

use crate::accounts::{AccountsEvent, AccountsView};
use crate::chat::{ChatEvent, ChatView, TabStatus, TabSummary};
use crate::palette::{Command, Entry, Group};
use crate::palette_view::{PaletteEvent, PaletteView};
use crate::theme::{self, ThemeChoice};
use crate::settings::{SettingsEvent, SettingsView};

/// The session directory has no change notification we rely on; a scan reads
/// at most 4 KiB per file, so polling is cheap. Tabs also trigger a rescan
/// when their session file appears or a turn settles.
const RESCAN_EVERY: Duration = Duration::from_secs(3);
/// Sessions shown per sidebar folder before "show more".
const GROUP_PREVIEW_SESSIONS: usize = 5;

actions!(
	workspace,
	[
		Hide,
		HideOthers,
		ShowAll,
		Quit,
		NewTab,
		NewTabInDir,
		CloseTab,
		SearchSessions,
		ToggleSidebar,
		TogglePanel,
		CommandPalette,
		ModelPalette,
		CycleThinking,
		OpenAccounts,
		OpenSettings,
		MinimizeWindow,
		ZoomWindow,
		ToggleFullScreen,
		NextTab,
		PreviousTab,
	]
);

/// Registers the native application menu bar (including the `Window` menu
/// that macOS populates with window controls and open windows) and global
/// app-level actions (`⌘Q`, `⌘H`, `⌥⌘H`).
pub fn init_menus(cx: &mut App) {
	use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};

	cx.on_action(|_: &Hide, cx| cx.hide());
	cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
	cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
	cx.on_action(|_: &Quit, cx| cx.quit());

	cx.bind_keys([
		KeyBinding::new("cmd-h", Hide, None),
		KeyBinding::new("alt-cmd-h", HideOthers, None),
		KeyBinding::new("cmd-q", Quit, None),
		KeyBinding::new("cmd-n", NewTab, None),
		KeyBinding::new("cmd-o", NewTabInDir, None),
		KeyBinding::new("cmd-w", CloseTab, None),
		KeyBinding::new("cmd-p", SearchSessions, None),
		KeyBinding::new("cmd-b", ToggleSidebar, None),
		KeyBinding::new("cmd-j", TogglePanel, None),
		KeyBinding::new("cmd-k", CommandPalette, None),
		KeyBinding::new("cmd-m", ModelPalette, None),
		KeyBinding::new("cmd-t", CycleThinking, None),
		KeyBinding::new("cmd-shift-l", OpenAccounts, None),
		KeyBinding::new("cmd-,", OpenSettings, None),
		KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
		KeyBinding::new("cmd-shift-]", NextTab, None),
		KeyBinding::new("cmd-shift-[", PreviousTab, None),
	]);

	cx.set_menus(vec![
		Menu::new("omp-desktop").items([
			MenuItem::action("设置…", OpenSettings),
			MenuItem::separator(),
			MenuItem::os_submenu("服务", SystemMenuType::Services),
			MenuItem::separator(),
			MenuItem::action("隐藏 omp-desktop", Hide),
			MenuItem::action("隐藏其他", HideOthers),
			MenuItem::action("全部显示", ShowAll),
			MenuItem::separator(),
			MenuItem::action("退出 omp-desktop", Quit),
		]),
		Menu::new("File").items([
			MenuItem::action("新建会话（当前目录）", NewTab),
			MenuItem::action("选择目录新建会话…", NewTabInDir),
			MenuItem::separator(),
			MenuItem::action("搜索会话", SearchSessions),
			MenuItem::separator(),
			MenuItem::action("关闭会话", CloseTab),
		]),
		Menu::new("Edit").items([
			MenuItem::os_action("撤销", Undo, OsAction::Undo),
			MenuItem::os_action("重做", Redo, OsAction::Redo),
			MenuItem::separator(),
			MenuItem::os_action("剪切", Cut, OsAction::Cut),
			MenuItem::os_action("拷贝", Copy, OsAction::Copy),
			MenuItem::os_action("粘贴", Paste, OsAction::Paste),
			MenuItem::os_action("全选", SelectAll, OsAction::SelectAll),
		]),
		Menu::new("View").items([
			MenuItem::action("命令面板…", CommandPalette),
			MenuItem::action("切换模型…", ModelPalette),
			MenuItem::action("循环思考档位", CycleThinking),
			MenuItem::separator(),
			MenuItem::action("显示 / 隐藏会话栏", ToggleSidebar),
			MenuItem::action("状态面板", TogglePanel),
			MenuItem::separator(),
			MenuItem::action("账号与登录…", OpenAccounts),
		]),
		Menu::new("Window").items([
			MenuItem::action("最小化", MinimizeWindow),
			MenuItem::action("缩放", ZoomWindow),
			MenuItem::action("进入全屏幕", ToggleFullScreen),
			MenuItem::separator(),
			MenuItem::action("显示上一个标签页", PreviousTab),
			MenuItem::action("显示下一个标签页", NextTab),
		]),
	]);
}

pub struct Workspace {
	/// Launch settings shared by every tab (`omp` path, `--session-dir`,
	/// extra flags); `cwd` and `resume` are set per tab.
	base:            SessionOptions,
	default_cwd:     PathBuf,
	/// Where saved sessions are listed from.
	root:            Option<PathBuf>,
	tabs:            Vec<Tab>,
	active:          usize,
	scan:            Scan,
	search:          Entity<InputState>,
	sidebar:         bool,
	/// Sidebar groups the user unfolded, by cwd. Groups start folded.
	unfolded:        HashSet<PathBuf>,
	/// Sidebar groups expanded past [`GROUP_PREVIEW_SESSIONS`], by cwd.
	expanded_groups: HashSet<PathBuf>,
	/// The accounts modal, while open; it runs its own omp process.
	accounts:        Option<(Entity<AccountsView>, Subscription)>,
	/// The command palette (⌘K, or ⌘M for models only), while open.
	palette:         Option<(Entity<PaletteView>, Subscription)>,
	/// The settings modal (⌘,), while open.
	settings:        Option<(Entity<SettingsView>, Subscription)>,
	_tasks:          Vec<Task<()>>,
	_subs:           Vec<Subscription>,
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
			unfolded: HashSet::new(),
			expanded_groups: HashSet::new(),
			accounts: None,
			palette: None,
			settings: None,
			_tasks: Vec::new(),
			_subs: vec![sub],
		};
		// "Follow the system" tracks appearance changes while running.
		let appearance = cx.observe_window_appearance(window, |_, window, cx| {
			theme::appearance_changed(window, cx);
		});
		this._subs.push(appearance);
		// No tab at launch: starting omp is slow, so wait until the user opens
		// a session. The search box takes focus so ⌘ shortcuts work at once.
		this.search.update(cx, |state, cx| state.focus(window, cx));
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

	/// Opens the native folder picker and starts a new session in the chosen directory.
	pub fn prompt_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let rx = cx.prompt_for_paths(PathPromptOptions {
			files:       false,
			directories: true,
			multiple:    false,
			prompt:      Some("选择工作目录".into()),
		});
		cx.spawn_in(window, async move |this, cx| {
			if let Ok(Ok(Some(mut paths))) = rx.await
				&& let Some(dir) = paths.pop()
			{
				let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
				_ = this.update_in(cx, |this, window, cx| this.new_tab(dir, window, cx));
			}
		})
		.detach();
	}

	/// Unique working directories known to the workspace (active tab first, then recent sessions).
	fn known_cwds(&self) -> Vec<PathBuf> {
		let mut cwds = Vec::new();
		let mut push = |path: &Path| {
			if !cwds.iter().any(|p| p == path) {
				cwds.push(path.to_path_buf());
			}
		};
		if let Some(tab) = self.tabs.get(self.active) {
			push(&tab.summary.cwd);
		}
		for tab in &self.tabs {
			push(&tab.summary.cwd);
		}
		push(&self.default_cwd);
		for session in &self.scan.sessions {
			push(&session.cwd);
		}
		cwds
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

	/// Closes a tab and stops its omp process. Closing the last one leaves the
	/// empty state rather than starting another omp.
	pub fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
		if index >= self.tabs.len() {
			return;
		}
		let tab = self.tabs.remove(index);
		tab.view.update(cx, |view, cx| view.shutdown(cx));
		if self.tabs.is_empty() {
			self.active = 0;
			self.search.update(cx, |state, cx| state.focus(window, cx));
			cx.notify();
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

	pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		if self.settings.is_some() {
			return;
		}
		self.palette = None;
		let cwd = self.tabs.get(self.active).map_or(self.default_cwd.clone(), |tab| tab.summary.cwd.clone());
		let recent = self.known_cwds();
		let view = cx.new(|cx| SettingsView::new(&self.base, cwd, recent, window, cx));
		let sub = cx.subscribe_in(&view, window, |this, _, event: &SettingsEvent, window, cx| match event {
			SettingsEvent::Close => this.close_settings(window, cx),
			SettingsEvent::DefaultCwd(dir) => {
				this.default_cwd = dir.clone().or_else(crate::launch_env::home_dir).unwrap_or_else(|| this.default_cwd.clone());
				cx.notify();
			},
		});
		self.settings = Some((view, sub));
		cx.notify();
	}

	fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		if self.settings.take().is_some()
			&& let Some(view) = self.tabs.get(self.active).map(|tab| tab.view.clone())
		{
			view.update(cx, |view, cx| view.focus_composer(window, cx));
		}
		cx.notify();
	}

	/// ⌘K: window actions plus the active tab's models, levels, commands.
	/// ⌘M: the same palette narrowed to models.
	pub fn open_palette(&mut self, models_only: bool, window: &mut Window, cx: &mut Context<Self>) {
		if self.palette.is_some() || self.accounts.is_some() || self.settings.is_some() {
			return;
		}
		let tab = self.tabs.get(self.active).map(|tab| tab.view.read(cx).palette_entries()).unwrap_or_default();
		let entries: Vec<Entry> = if models_only {
			tab.into_iter().filter(|e| e.group == Group::Model).collect()
		} else {
			let theme = theme::choice(cx);
			let mut entries = vec![
				Entry::new(Group::Action, "新建会话（当前目录）", Command::NewTab).detail("new tab").shortcut("⌘N"),
				Entry::new(Group::Action, "选择目录新建会话…", Command::NewTabInDir).detail("new tab directory folder open").shortcut("⌘O"),
				Entry::new(Group::Action, "关闭会话", Command::CloseTab).detail("close tab").shortcut("⌘W"),
				Entry::new(Group::Action, "搜索会话", Command::SearchSessions).detail("search sessions").shortcut("⌘P"),
				Entry::new(Group::Action, "显示 / 隐藏会话栏", Command::ToggleSidebar).detail("sidebar").shortcut("⌘B"),
				Entry::new(Group::Action, "状态面板", Command::TogglePanel).detail("status panel").shortcut("⌘J"),
				Entry::new(Group::Action, "循环思考档位", Command::CycleThinking).detail("cycle thinking").shortcut("⌘T"),
				Entry::new(Group::Action, "压缩上下文", Command::Compact).detail("compact"),
				Entry::new(Group::Action, "账号与登录", Command::Accounts).detail("accounts login logout").shortcut("⌘⇧L"),
				Entry::new(Group::Action, "设置", Command::Settings).detail("settings preferences launch directory cwd theme packages install").shortcut("⌘,"),
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
			Command::NewTabInDir => self.prompt_new_tab(window, cx),
			Command::CloseTab => self.close_tab(self.active, window, cx),
			Command::SearchSessions => self.focus_search(window, cx),
			Command::ToggleSidebar => {
				self.sidebar = !self.sidebar;
				cx.notify();
			},
			Command::Accounts => self.open_accounts(window, cx),
			Command::Settings => self.open_settings(window, cx),
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
	pub(crate) fn settings_view(&self) -> Option<Entity<SettingsView>> {
		self.settings.as_ref().map(|(view, _)| view.clone())
	}

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
		if keystroke.key == "escape" && self.settings.is_some() {
			self.close_settings(window, cx);
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
			"o" => self.prompt_new_tab(window, cx),
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

	fn render_tab_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
			let active = index == self.active;
			let (dot, badge) = status_style(tab.summary.status, p);
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
			// The default left padding clears the traffic lights, which macOS
			// hides in fullscreen; the bar adds its own small inset there.
			.when(window.is_fullscreen(), |bar| bar.pl(px(0.)))
			.child(
				div()
					.flex_1()
					.min_w_0()
					.h_full()
					.flex()
					.items_center()
					.gap_1()
					.children(tabs)
					.child(self.render_new_session_menu("tab-new", true, Anchor::TopLeft, cx))
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
					Button::new("settings-open")
						.ghost()
						.xsmall()
						.label("设置")
						.tooltip("启动目录、外观、omp 包（⌘,）")
						.on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
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

	fn render_new_session_menu(
		&self,
		id: &'static str,
		xsmall: bool,
		anchor: Anchor,
		cx: &mut Context<Self>,
	) -> impl IntoElement {
		let this = cx.entity().downgrade();
		let cwds = self.known_cwds();
		let btn = Button::new(id).ghost().label("+").tooltip("新建会话（可选择目录）");
		let btn = if xsmall { btn.xsmall() } else { btn.small() };
		btn.dropdown_menu_with_anchor(anchor, move |menu, _, _| {
			let this_pick = this.clone();
			let mut menu = menu
				.scrollable(true)
				.max_w(px(340.))
				.max_h(px(360.))
				.item(
					PopupMenuItem::new("选择目录新建会话…")
						.icon(IconName::FolderOpen)
						.on_click(move |_, window, cx| {
							_ = this_pick.update(cx, |this, cx| this.prompt_new_tab(window, cx));
						}),
				)
				.item(PopupMenuItem::separator())
				.item(PopupMenuItem::label("在已有目录新建"));
			for cwd in &cwds {
				let cwd = cwd.clone();
				let this_dir = this.clone();
				let name = folder_name(&cwd);
				let path_str = display_path(&cwd);
				menu = menu.item(
					PopupMenuItem::element(move |_, cx| {
						let p = theme::palette(cx);
						div()
							.w(px(296.))
							.max_w(px(296.))
							.overflow_hidden()
							.flex()
							.items_center()
							.justify_between()
							.gap_2()
							.child(div().flex_none().max_w(px(130.)).truncate().font_weight(FontWeight::MEDIUM).child(name.clone()))
							.child(div().flex_1().min_w_0().truncate().text_right().text_xs().text_color(rgb(p.muted_foreground)).child(path_str.clone()))
					})
					.icon(IconName::Folder)
					.on_click(move |_, window, cx| {
						let cwd = cwd.clone();
						_ = this_dir.update(cx, |this, cx| this.new_tab(cwd, window, cx));
					}),
				);
			}
			menu
		})
	}

	/// Shown when no tab is open: start a session in the default directory,
	/// or pick one from the sidebar.
	fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let cwd = self.default_cwd.clone();
		div()
			.id("empty-state")
			.flex_1()
			.min_w_0()
			.h_full()
			.flex()
			.flex_col()
			.items_center()
			.justify_center()
			.gap_3()
			.child(div().text_color(rgb(p.muted_foreground)).child("从左侧选择会话继续，或新建会话"))
			.child(
				Button::new("empty-new")
					.primary()
					.label(format!("在 {} 新建会话  ⌘N", display_path(&cwd)))
					.on_click(cx.listener(move |this, _, window, cx| this.new_tab(cwd.clone(), window, cx))),
			)
	}

	fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let query = self.search.read(cx).value().to_string();
		let searching = !query.trim().is_empty();
		let groups = sessions::group(&self.scan.sessions, &query);
		let active_tab = self.tabs.get(self.active);
		let selected = active_tab.and_then(|tab| tab.summary.session_file.clone());
		// Sessions open in a tab, with that tab's status, so the list mirrors the tab bar.
		let open: HashMap<&Path, TabStatus> = self
			.tabs
			.iter()
			.filter_map(|tab| tab.summary.session_file.as_deref().map(|file| (file, tab.summary.status)))
			.collect();
		let active_cwd = active_tab.map(|tab| tab.summary.cwd.as_path());
		let now = SystemTime::now();
		let mut row = 0usize;
		let mut items: Vec<AnyElement> = Vec::new();
		for (gi, group) in groups.iter().enumerate() {
			let cwd = group.cwd.to_owned();
			let folded = !searching && !self.unfolded.contains(&cwd);
			let is_active_group = active_cwd == Some(group.cwd);
			let (toggle, new_cwd) = (cwd.clone(), cwd.clone());
			let icon = if folded { IconName::FolderClosed } else { IconName::FolderOpen };
			let icon_color = if is_active_group { p.primary } else { p.muted_foreground };
			items.push(
				div()
					.id(("group", gi))
					.flex()
					.items_center()
					.gap_2()
					.mt(if gi == 0 { px(2.) } else { px(8.) })
					.px_2p5()
					.h(px(34.))
					.rounded(px(8.))
					.cursor_pointer()
					.hover(|s| s.bg(rgb(p.muted)))
					.child(Icon::new(icon).small().text_color(rgb(icon_color)))
					.child(
						div()
							.flex_1()
							.min_w_0()
							.truncate()
							.text_sm()
							.font_weight(FontWeight::MEDIUM)
							.text_color(rgb(p.foreground))
							.child(folder_name(&cwd)),
					)
					.child(Button::new(("group-new", gi)).ghost().xsmall().label("+").tooltip(format!("在 {} 新建会话", display_path(&cwd))).on_click(
						cx.listener(move |this, _, window, cx| {
							cx.stop_propagation();
							this.new_tab(new_cwd.clone(), window, cx);
						}),
					))
					.on_click(cx.listener(move |this, _, _, cx| {
						if !this.unfolded.remove(&toggle) {
							this.unfolded.insert(toggle.clone());
						}
						cx.notify();
					}))
					.test_support()
					.into_any_element(),
			);
			// A folded group lists only its sessions with a run in progress
			// (including one waiting on a dialog).
			if folded {
				for session in &group.sessions {
					let status = open.get(session.path.as_path()).copied();
					if matches!(status, Some(TabStatus::Running | TabStatus::Attention)) {
						items.push(self.render_session(row, session, selected.as_deref(), status, now, p, cx));
						row += 1;
					}
				}
				continue;
			}
			let expanded = searching || self.expanded_groups.contains(&cwd);
			// Folded previews still list sessions open in a tab.
			let mut hidden = 0usize;
			for (i, session) in group.sessions.iter().enumerate() {
				let status = open.get(session.path.as_path()).copied();
				if !expanded && i >= GROUP_PREVIEW_SESSIONS && status.is_none() {
					hidden += 1;
					continue;
				}
				items.push(self.render_session(row, session, selected.as_deref(), status, now, p, cx));
				row += 1;
			}
			if !searching && (hidden > 0 || (expanded && group.sessions.len() > GROUP_PREVIEW_SESSIONS)) {
				let more_cwd = cwd.clone();
				items.push(
					div()
						.id(("group-more", gi))
						.flex()
						.items_center()
						.h(px(28.))
						.pl_7()
						.pr_3()
						.my_0p5()
						.rounded(px(8.))
						.text_xs()
						.text_color(rgb(p.muted_foreground))
						.cursor_pointer()
						.hover(|s| s.bg(rgb(p.muted)).text_color(rgb(p.foreground)))
						.child(if expanded { "收起".to_owned() } else { format!("展开其余 {hidden} 个") })
						.on_click(cx.listener(move |this, _, _, cx| {
							if !this.expanded_groups.remove(&more_cwd) {
								this.expanded_groups.insert(more_cwd.clone());
							}
							cx.notify();
						}))
						.test_support()
						.into_any_element(),
				);
			}
		}
		let empty = items.is_empty();
		div()
			.id("sidebar")
			.w(px(304.))
			.flex_none()
			.h_full()
			.flex()
			.flex_col()
			.border_r_1()
			.border_color(rgb(p.border))
			.bg(rgb(p.panel))
			.child(
				div()
					.p_2p5()
					.flex()
					.items_center()
					.gap_1p5()
					.child(div().id("session-search").flex_1().child(Input::new(&self.search).small().cleanable(true)).test_support())
					.child(self.render_new_session_menu("sidebar-new", false, Anchor::TopRight, cx)),
			)
			.child(
				div()
					.id("session-list")
					.flex_1()
					.min_h_0()
					.overflow_y_scroll()
					.px_2()
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

	fn render_session(
		&self,
		row: usize,
		session: &SessionInfo,
		selected: Option<&Path>,
		open: Option<TabStatus>,
		now: SystemTime,
		p: &theme::Palette,
		cx: &mut Context<Self>,
	) -> AnyElement {
		let is_selected = selected == Some(session.path.as_path());
		let (path, cwd) = (session.path.clone(), session.cwd.clone());
		let (fork_path, fork_cwd, reveal) = (path.clone(), cwd.clone(), path.clone());
		let this = cx.entity().downgrade();
		let title = session.title.clone().unwrap_or_else(|| "（未命名）".into());
		let selected_bg = if cx.theme().is_dark() { p.muted } else { p.accent };
		div()
			.id(("session-row", row))
			.flex()
			.items_center()
			.gap_2()
			.h(px(34.))
			.pl_2()
			.pr_3()
			.my_0p5()
			.rounded(px(8.))
			.text_sm()
			.cursor_pointer()
			.when(is_selected, |r| {
				r.bg(rgb(selected_bg))
					.text_color(rgb(p.foreground))
					.font_weight(FontWeight::MEDIUM)
			})
			.when(!is_selected, |r| r.hover(|s| s.bg(rgb(p.muted))))
			// Same dot as the tab bar while the session is open in a tab.
			.child(
				div().flex_none().w(px(12.)).flex().justify_center().when_some(open, |slot, status| {
					slot.child(div().size(px(7.)).rounded_full().bg(rgb(status_style(status, p).0)))
				}),
			)
			.child(div().flex_1().min_w_0().truncate().child(title))
			.when_some(open.map(|status| status_style(status, p)), |row, (color, badge)| {
				row.when_some(badge, |row, badge| row.child(div().flex_none().text_xs().text_color(rgb(color)).child(badge)))
			})
			.child(
				div()
					.flex_none()
					.text_xs()
					.text_color(rgb(p.muted_foreground))
					.child(sessions::relative_age(session.modified, now)),
			)
			.on_click(cx.listener(move |this, _, window, cx| this.open_session(&path, &cwd, false, window, cx)))
			.test_support()
			// Right-click: continue in a copy (omp `--fork`), or show the file.
			.context_menu(move |menu, _, _| {
				let this = this.clone();
				let (fork_path, fork_cwd, reveal) = (fork_path.clone(), fork_cwd.clone(), reveal.clone());
				menu.item(PopupMenuItem::new("分支：复制为新会话继续").on_click(move |_, window, cx| {
					_ = this.update(cx, |this, cx| this.open_session(&fork_path, &fork_cwd, true, window, cx));
				}))
				.item(PopupMenuItem::new("在 Finder 中显示").on_click(move |_, _, cx| cx.reveal_path(&reveal)))
			})
			.into_any_element()
	}
}

impl Render for Workspace {
	fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
			.on_action(cx.listener(|this, _: &NewTab, window, cx| this.run_command(Command::NewTab, window, cx)))
			.on_action(cx.listener(|this, _: &NewTabInDir, window, cx| this.run_command(Command::NewTabInDir, window, cx)))
			.on_action(cx.listener(|this, _: &CloseTab, window, cx| this.run_command(Command::CloseTab, window, cx)))
			.on_action(cx.listener(|this, _: &SearchSessions, window, cx| this.run_command(Command::SearchSessions, window, cx)))
			.on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| this.run_command(Command::ToggleSidebar, window, cx)))
			.on_action(cx.listener(|this, _: &TogglePanel, window, cx| this.run_command(Command::TogglePanel, window, cx)))
			.on_action(cx.listener(|this, _: &CommandPalette, window, cx| this.open_palette(false, window, cx)))
			.on_action(cx.listener(|this, _: &ModelPalette, window, cx| this.open_palette(true, window, cx)))
			.on_action(cx.listener(|this, _: &CycleThinking, window, cx| this.run_command(Command::CycleThinking, window, cx)))
			.on_action(cx.listener(|this, _: &OpenAccounts, window, cx| this.open_accounts(window, cx)))
			.on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
			.on_action(cx.listener(|_, _: &MinimizeWindow, window, _| window.minimize_window()))
			.on_action(cx.listener(|_, _: &ZoomWindow, window, _| window.zoom_window()))
			.on_action(cx.listener(|_, _: &ToggleFullScreen, window, _| window.toggle_fullscreen()))
			.on_action(cx.listener(|this, _: &NextTab, window, cx| {
				if !this.tabs.is_empty() {
					this.activate((this.active + 1) % this.tabs.len(), window, cx);
				}
			}))
			.on_action(cx.listener(|this, _: &PreviousTab, window, cx| {
				if !this.tabs.is_empty() {
					this.activate((this.active + this.tabs.len() - 1) % this.tabs.len(), window, cx);
				}
			}))
			.child(self.render_tab_bar(window, cx))
			.child(
				div()
					.flex_1()
					.min_h_0()
					.flex()
					.when(self.sidebar, |row| row.child(self.render_sidebar(cx)))
					.child(match active {
						Some(view) => div().flex_1().min_w_0().h_full().relative().child(view).into_any_element(),
						None => self.render_empty(cx).into_any_element(),
					}),
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
			.when_some(self.settings.as_ref().map(|(view, _)| view.clone()), |root, view| {
				root.child(
					div()
						.id("settings-overlay")
						.absolute()
						.inset_0()
						.flex()
						.items_center()
						.justify_center()
						.bg(hsla(0., 0., 0., 0.35))
						.on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_settings(window, cx)))
						.child(view),
				)
			})
	}
}

/// Dot color and optional badge for a tab's status, shared by the tab bar
/// and the sidebar rows of open sessions.
fn status_style(status: TabStatus, p: &theme::Palette) -> (u32, Option<&'static str>) {
	match status {
		TabStatus::Connecting => (p.warning, None),
		TabStatus::Idle => (p.success, None),
		TabStatus::Running => (p.primary, Some("•••")),
		TabStatus::Attention => (p.warning, Some("待确认")),
		TabStatus::Disconnected => (p.danger, Some("已断开")),
	}
}

/// Directory basename for folder headers (falling back to `display_path`).
fn folder_name(path: &Path) -> String {
	path.file_name()
		.and_then(|n| n.to_str())
		.filter(|s| !s.is_empty())
		.map_or_else(|| display_path(path), str::to_owned)
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

#[cfg(test)]
mod tests {
	use ::core::prelude::v1::test;
	use gpui_kit::test::TestWindowExt as _;

	use super::*;

	#[gpui_kit::test]
	async fn sidebar_folder_clips_past_five_sessions_and_toggles(cx: &mut TestAppContext) {
		cx.executor().allow_parking();
		cx.update(gpui_kit::init);
		cx.update(crate::theme::apply);
		cx.update(init_menus);
		let menus = cx.update(|cx| cx.get_menus().unwrap_or_default());
		assert!(menus.iter().any(|m| m.name.as_ref() == "Window"), "Window menu registered for macOS window controls");

		let dir = std::env::temp_dir().join(format!("omp-desktop-sidebar-test-{}", std::process::id()));
		_ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		let base = SessionOptions { omp: PathBuf::from("/nonexistent-omp"), session_dir: Some(dir.clone()), ..Default::default() };
		let cwd_a = PathBuf::from("/tmp/proj-a");
		let cwd_b = PathBuf::from("/tmp/proj-b");
		let make_session = |id: usize, cwd: &Path| SessionInfo {
			id:       format!("s{id}"),
			path:     dir.join(format!("s{id}.jsonl")),
			cwd:      cwd.to_owned(),
			title:    Some(format!("会话 {id}")),
			created:  None,
			parent:   None,
			modified: SystemTime::UNIX_EPOCH + Duration::from_secs(100 + id as u64),
		};
		let mut sessions: Vec<SessionInfo> = (1..=7).map(|i| make_session(i, &cwd_a)).collect();
		sessions.extend((8..=10).map(|i| make_session(i, &cwd_b)));

		let (handle, view) = cx.update(|cx| {
			gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| cx.new(|cx| Workspace::new(base, cwd_a.clone(), window, cx))).unwrap()
		});
		view.update(cx, |this, cx| {
			this.scan = Scan { sessions, skipped: 0 };
			cx.notify();
		});
		cx.update_window(handle, |_, window, cx| {
			window.render_frame(cx);
			// Groups start folded, and no session is running, so no rows.
			assert!(window.try_find(("session-row", 0usize)).is_none(), "folded groups hide idle sessions");
			window.click(("group", 0usize), cx);
			window.click(("group", 1usize), cx);
			// Group 0 (proj-a, 7 sessions) shows 5 + toggle; group 1 (proj-b, 3 sessions) has no toggle.
			assert!(window.try_find(("group-more", 0usize)).is_some(), "7 sessions should show toggle");
			assert!(window.try_find(("group-more", 1usize)).is_none(), "3 sessions should not show toggle");
			assert!(window.try_find(("session-row", 7usize)).is_some(), "5 + 3 = 8 rows (0..=7) visible initially");
			assert!(window.try_find(("session-row", 8usize)).is_none(), "9th row hidden until expanded");

			window.click(("group-more", 0usize), cx);
			assert!(window.try_find(("session-row", 9usize)).is_some(), "all 10 rows (0..=9) visible once expanded");

			window.click(("group-more", 0usize), cx);
			assert!(window.try_find(("session-row", 8usize)).is_none(), "collapsed back to 5 sessions");
		})
		.unwrap();
		_ = std::fs::remove_dir_all(&dir);
		_ = std::fs::remove_dir_all(&dir);
	}

	/// ⌘, opens settings; the nav switches pages; a theme card applies the
	/// theme; the package page lists through `omp list` (a stub script here),
	/// installs and removes after confirming; Esc closes.
	#[gpui_kit::test]
	async fn settings_panel_pages_theme_and_packages(cx: &mut TestAppContext) {
		use std::os::unix::fs::PermissionsExt as _;

		use crate::settings::Page;
		use crate::theme::ThemeChoice;

		cx.executor().allow_parking();
		cx.update(gpui_kit::init);
		cx.update(crate::theme::apply);
		cx.update(init_menus);

		let dir = std::env::temp_dir().join(format!("omp-desktop-settings-test-{}", std::process::id()));
		_ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		// A stand-in `omp` that keeps its package list in a file.
		let state = dir.join("packages");
		std::fs::write(&state, "npm:@foo/bar\n").unwrap();
		let omp = dir.join("omp");
		std::fs::write(
			&omp,
			format!(
				"#!/bin/sh\nS='{}'\ncase \"$1\" in\n list) echo 'User packages:'; while read -r l; do echo \"  $l\"; done < \"$S\";;\n install) echo \"$2\" >> \"$S\";;\n remove) grep -v -x \"$2\" \"$S\" > \"$S.t\"; mv \"$S.t\" \"$S\";;\n update) ;;\n *) exit 1;;\nesac\n",
				state.display()
			),
		)
		.unwrap();
		std::fs::set_permissions(&omp, std::fs::Permissions::from_mode(0o755)).unwrap();

		let base = SessionOptions { omp, session_dir: Some(dir.clone()), ..Default::default() };
		let cwd = dir.clone();
		let (handle, workspace) = cx.update(|cx| {
			gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| cx.new(|cx| Workspace::new(base, cwd, window, cx))).unwrap()
		});
		cx.update_window(handle, |_, window, cx| {
			window.render_frame(cx);
			window.click("settings-open", cx);
			window.render_frame(cx);
		})
		.unwrap();
		let settings = workspace.read_with(cx, |w, _| w.settings_view()).expect("settings opened");
		assert_eq!(settings.read_with(cx, |v, _| v.page()), Page::General);

		cx.update_window(handle, |_, window, cx| {
			assert!(window.try_find("settings-cwd").is_some());
			window.click("settings-nav-appearance", cx);
			window.render_frame(cx);
			window.click("settings-theme-dark", cx);
			window.render_frame(cx);
		})
		.unwrap();
		assert_eq!(cx.update(|cx| crate::theme::choice(cx)), ThemeChoice::Dark);

		cx.update_window(handle, |_, window, cx| {
			window.click("settings-nav-packages", cx);
			window.render_frame(cx);
		})
		.unwrap();
		let wait = |cx: &mut TestAppContext, what: &str, done: &dyn Fn(&crate::settings::SettingsView) -> bool| {
			let deadline = std::time::Instant::now() + Duration::from_secs(10);
			while !settings.read_with(cx, |v, _| done(v)) {
				assert!(std::time::Instant::now() < deadline, "timed out waiting for: {what}");
				cx.run_until_parked();
				std::thread::sleep(Duration::from_millis(20));
			}
		};
		wait(cx, "listed", &|v| !v.busy() && v.packages().is_some_and(|p| p.len() == 1));

		cx.update_window(handle, |_, window, cx| {
			settings.update(cx, |v, cx| v.install_source("git:github.com/u/r", window, cx));
		})
		.unwrap();
		wait(cx, "installed", &|v| !v.busy() && v.packages().is_some_and(|p| p.len() == 2));
		assert!(matches!(settings.read_with(cx, |v, _| v.notice().cloned()), Some(crate::settings::Notice::Info(_))));

		cx.update_window(handle, |_, window, cx| {
			window.render_frame(cx);
			window.click(("package-remove", 0usize), cx);
			window.render_frame(cx);
			window.click(("package-confirm", 0usize), cx);
			window.render_frame(cx);
		})
		.unwrap();
		wait(cx, "removed", &|v| !v.busy() && v.packages().is_some_and(|p| p.len() == 1));
		assert_eq!(settings.read_with(cx, |v, _| v.packages().unwrap()[0].source.clone()), "git:github.com/u/r");

		cx.update_window(handle, |_, window, cx| window.press("escape", cx)).unwrap();
		assert!(workspace.read_with(cx, |w, _| w.settings_view()).is_none(), "Esc closes settings");
		_ = std::fs::remove_dir_all(&dir);
	}
}
