//! Settings panel (⌘,): the launch directory, the theme, and omp packages.
//! Every change applies at once; there is nothing to save.
//!
//! Package operations shell out to `omp list / install / remove / update`
//! (see `packages`). They change omp's settings, so open sessions keep what
//! they loaded and new sessions pick up the change.

use std::path::{Path, PathBuf};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use omp_bridge::SessionOptions;

use crate::launch_env;
use crate::packages::{self, Package, Scope};
use crate::theme::{self, ThemeChoice};

pub enum SettingsEvent {
	/// The launch directory changed: `Some` is the new one, `None` means home.
	DefaultCwd(Option<PathBuf>),
	Close,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
	General,
	Appearance,
	Packages,
}

impl Page {
	const ALL: [Self; 3] = [Self::General, Self::Appearance, Self::Packages];

	fn label(self) -> &'static str {
		match self {
			Self::General => "启动目录",
			Self::Appearance => "外观",
			Self::Packages => "omp 包",
		}
	}

	fn id(self) -> &'static str {
		match self {
			Self::General => "settings-nav-general",
			Self::Appearance => "settings-nav-appearance",
			Self::Packages => "settings-nav-packages",
		}
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
	Info(String),
	Error(String),
}

/// What a running `omp` package command is doing, for the busy label.
#[derive(Clone, Debug, PartialEq)]
enum Job {
	Listing,
	Installing(String),
	Updating(Option<String>),
	Removing(String),
}

impl Job {
	fn label(&self) -> String {
		match self {
			Self::Listing => "正在读取已安装的包…".into(),
			Self::Installing(source) => format!("正在安装 {source}…"),
			Self::Updating(Some(source)) => format!("正在更新 {source}…"),
			Self::Updating(None) => "正在更新全部包…".into(),
			Self::Removing(source) => format!("正在移除 {source}…"),
		}
	}
}

pub struct SettingsView {
	options:    SessionOptions,
	/// Where `omp` package commands run; project packages come from here.
	cwd:        PathBuf,
	page:       Page,
	/// The saved launch directory; `None` launches in home.
	saved_cwd:  Option<PathBuf>,
	recent:     Vec<PathBuf>,
	packages:   Option<Vec<Package>>,
	job:        Option<Job>,
	/// Package waiting for the user to confirm its removal.
	confirm:    Option<String>,
	notice:     Option<Notice>,
	install:    Entity<InputState>,
	_subs:      Vec<Subscription>,
}

impl SettingsView {
	pub fn new(options: &SessionOptions, cwd: PathBuf, recent: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let install = cx.new(|cx| InputState::new(window, cx).placeholder("npm:@scope/name、git:github.com/user/repo 或本地路径"));
		let subs = vec![cx.subscribe_in(&install, window, |this, _, event: &InputEvent, window, cx| match event {
			InputEvent::PressEnter { .. } => this.install(window, cx),
			InputEvent::Change => cx.notify(),
			_ => {},
		})];
		Self {
			options: options.clone(),
			cwd,
			page: Page::General,
			saved_cwd: launch_env::configured_cwd(),
			recent,
			packages: None,
			job: None,
			confirm: None,
			notice: None,
			install,
			_subs: subs,
		}
	}

	#[cfg(test)]
	pub fn page(&self) -> Page {
		self.page
	}

	#[cfg(test)]
	pub fn packages(&self) -> Option<&[Package]> {
		self.packages.as_deref()
	}

	#[cfg(test)]
	pub fn busy(&self) -> bool {
		self.job.is_some()
	}

	#[cfg(test)]
	pub fn notice(&self) -> Option<&Notice> {
		self.notice.as_ref()
	}

	pub fn show(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
		self.page = page;
		self.notice = None;
		self.confirm = None;
		if page == Page::Packages {
			if self.packages.is_none() && self.job.is_none() {
				self.refresh(cx);
			}
			self.install.update(cx, |state, cx| state.focus(window, cx));
		}
		cx.notify();
	}

	// ---- launch directory ----

	fn set_cwd(&mut self, dir: Option<PathBuf>, cx: &mut Context<Self>) {
		match launch_env::save_default_cwd(dir.as_deref()) {
			Ok(()) => {
				self.notice = Some(Notice::Info(match &dir {
					Some(dir) => format!("启动目录已设为 {}", display_path(dir)),
					None => "已恢复默认：从 Finder 打开时使用主目录".into(),
				}));
				self.saved_cwd = dir.clone();
				cx.emit(SettingsEvent::DefaultCwd(dir));
			},
			Err(error) => self.notice = Some(Notice::Error(format!("保存启动目录失败：{error}"))),
		}
		cx.notify();
	}

	fn pick_cwd(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let rx = cx.prompt_for_paths(PathPromptOptions {
			files:       false,
			directories: true,
			multiple:    false,
			prompt:      Some("设为启动目录".into()),
		});
		cx.spawn_in(window, async move |this, cx| {
			if let Ok(Ok(Some(mut paths))) = rx.await
				&& let Some(dir) = paths.pop()
			{
				let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
				_ = this.update(cx, |this, cx| this.set_cwd(Some(dir), cx));
			}
		})
		.detach();
	}

	// ---- packages ----

	/// Runs one `omp` package command off the UI thread, then relists.
	fn run_job(&mut self, job: Job, cx: &mut Context<Self>, call: impl FnOnce(&SessionOptions, &Path) -> Result<String, String> + Send + 'static) {
		if self.job.is_some() {
			return;
		}
		self.job = Some(job);
		self.notice = None;
		self.confirm = None;
		let (options, cwd) = (self.options.clone(), self.cwd.clone());
		let task = cx.background_spawn(crate::blocking::blocking(move || {
			let done = call(&options, &cwd);
			let listed = packages::list(&options, &cwd);
			(done, listed)
		}));
		cx.spawn(async move |this, cx| {
			let (done, listed) = task.await;
			_ = this.update(cx, |this, cx| {
				this.job = None;
				match listed {
					Ok(list) => this.packages = Some(list),
					Err(error) => this.notice = Some(Notice::Error(format!("读取包列表失败：{error}"))),
				}
				match done {
					Ok(message) if !message.is_empty() => this.notice = Some(Notice::Info(message)),
					Ok(_) => {},
					Err(error) => this.notice = Some(Notice::Error(error)),
				}
				cx.notify();
			});
		})
		.detach();
		cx.notify();
	}

	pub fn refresh(&mut self, cx: &mut Context<Self>) {
		self.run_job(Job::Listing, cx, |_, _| Ok(String::new()));
	}

	fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let source = self.install.read(cx).value().trim().to_string();
		if source.is_empty() || self.job.is_some() {
			return;
		}
		let job = Job::Installing(source.clone());
		self.run_job(job, cx, move |options, cwd| {
			packages::install(options, cwd, &source).map(|()| format!("已安装 {source}，新开的会话生效"))
		});
		self.install.update(cx, |state, cx| state.set_value("", window, cx));
	}

	#[cfg(test)]
	pub fn install_source(&mut self, source: &str, window: &mut Window, cx: &mut Context<Self>) {
		self.install.update(cx, |state, cx| state.set_value(source, window, cx));
		self.install(window, cx);
	}

	fn update(&mut self, source: Option<String>, cx: &mut Context<Self>) {
		let job = Job::Updating(source.clone());
		self.run_job(job, cx, move |options, cwd| {
			packages::update(options, cwd, source.as_deref()).map(|()| match &source {
				Some(source) => format!("已更新 {source}，新开的会话生效"),
				None => "已更新全部包，新开的会话生效".into(),
			})
		});
	}

	fn remove(&mut self, package: Package, cx: &mut Context<Self>) {
		let job = Job::Removing(package.source.clone());
		self.run_job(job, cx, move |options, cwd| {
			packages::remove(options, cwd, &package).map(|()| format!("已移除 {}，新开的会话生效", package.source))
		});
	}
}

// ---- rendering ----

impl SettingsView {
	fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		div()
			.w(px(168.))
			.flex_none()
			.h_full()
			.flex()
			.flex_col()
			.gap_0p5()
			.p_2()
			.bg(rgb(p.panel))
			.border_r_1()
			.border_color(rgb(p.border))
			.child(div().px_2().pt_1().pb_2().text_base().font_weight(FontWeight::SEMIBOLD).child("设置"))
			.children(Page::ALL.map(|page| {
				let active = page == self.page;
				div()
					.id(page.id())
					.px_2()
					.h(px(28.))
					.flex()
					.items_center()
					.rounded(px(6.))
					.text_sm()
					.cursor_pointer()
					.when(active, |d| d.bg(rgb(p.accent)).text_color(rgb(p.accent_foreground)).font_weight(FontWeight::MEDIUM))
					.when(!active, |d| d.text_color(rgb(p.muted_foreground)).hover(|s| s.bg(rgb(p.muted))))
					.child(page.label())
					.on_click(cx.listener(move |this, _, window, cx| this.show(page, window, cx)))
					.test_support()
			}))
	}

	fn section(title: &str, detail: &str, p: &theme::Palette) -> Div {
		div()
			.flex()
			.flex_col()
			.gap_1()
			.child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title.to_owned()))
			.child(div().text_xs().text_color(rgb(p.muted_foreground)).child(detail.to_owned()))
	}

	fn render_general(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let saved = self.saved_cwd.clone();
		let current = match &saved {
			Some(dir) => display_path(dir),
			None => "~（主目录，默认）".into(),
		};
		let recent: Vec<AnyElement> = self
			.recent
			.iter()
			.filter(|dir| Some(dir.as_path()) != saved.as_deref())
			.take(8)
			.enumerate()
			.map(|(index, dir)| {
				let exists = dir.is_dir();
				let target = dir.clone();
				div()
					.id(("settings-recent", index))
					.flex()
					.items_center()
					.gap_2()
					.px_3()
					.h(px(32.))
					.border_b_1()
					.border_color(rgb(p.border))
					.text_sm()
					.hover(|s| s.bg(rgb(p.muted)))
					.child(
						div()
							.flex_1()
							.min_w_0()
							.truncate()
							.font_family("Menlo")
							.text_xs()
							.when(!exists, |d| d.text_color(rgb(p.muted_foreground)))
							.child(display_path(dir)),
					)
					.when(!exists, |r| r.child(div().text_xs().text_color(rgb(p.muted_foreground)).child("已不存在")))
					.when(exists, |r| {
						r.child(
							Button::new(("settings-recent-use", index))
								.xsmall()
								.ghost()
								.label("设为启动目录")
								.on_click(cx.listener(move |this, _, _, cx| this.set_cwd(Some(target.clone()), cx))),
						)
					})
					.into_any_element()
			})
			.collect();
		div()
			.flex()
			.flex_col()
			.gap_4()
			.child(Self::section(
				"启动目录",
				"从 Finder、Dock 或 open 打开 omp-desktop 时，首个会话和 ⌘N 在这里启动。从终端启动时仍使用终端的当前目录（或 --cwd）。",
				p,
			))
			.child(
				div()
					.flex()
					.items_center()
					.gap_2()
					.child(
						div()
							.id("settings-cwd")
							.flex_1()
							.min_w_0()
							.h(px(30.))
							.px_3()
							.flex()
							.items_center()
							.gap_2()
							.rounded(px(6.))
							.border_1()
							.border_color(rgb(p.border))
							.bg(rgb(p.background))
							.child(div().flex_1().min_w_0().truncate().font_family("Menlo").text_xs().child(current))
							.when(saved.is_some(), |d| d.child(div().text_xs().text_color(rgb(p.success)).child("● 已保存")))
							.test_support(),
					)
					.child(Button::new("settings-cwd-pick").small().outline().label("选择目录…").on_click(cx.listener(|this, _, window, cx| this.pick_cwd(window, cx))))
					.when(saved.is_some(), |d| {
						d.child(Button::new("settings-cwd-reset").small().ghost().label("恢复默认").on_click(cx.listener(|this, _, _, cx| this.set_cwd(None, cx))))
					}),
			)
			.when(!recent.is_empty(), |d| {
				d.child(div().pt_2().text_xs().font_weight(FontWeight::MEDIUM).text_color(rgb(p.muted_foreground)).child("最近使用的目录"))
					.child(div().rounded(px(6.)).border_1().border_color(rgb(p.border)).overflow_hidden().children(recent))
			})
	}

	fn render_appearance(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let current = theme::choice(cx);
		let card = |choice: ThemeChoice, cx: &mut Context<Self>| {
			let selected = choice == current;
			let (bg, panel, line) = match choice {
				ThemeChoice::Light => (theme::LIGHT.background, theme::LIGHT.panel, theme::LIGHT.border),
				ThemeChoice::Dark => (theme::DARK.background, theme::DARK.panel, theme::DARK.border),
				ThemeChoice::System => (theme::LIGHT.background, theme::DARK.panel, theme::DARK.border),
			};
			let preview = div()
				.h(px(72.))
				.rounded(px(4.))
				.border_1()
				.border_color(rgb(p.border))
				.overflow_hidden()
				.flex()
				.bg(rgb(bg))
				.child(div().w(px(36.)).h_full().bg(rgb(panel)).border_r_1().border_color(rgb(line)))
				.child(
					div()
						.flex_1()
						.p_2()
						.flex()
						.flex_col()
						.gap_1()
						.children([0.8, 0.55, 0.7].map(|w| div().h(px(4.)).w(relative(w)).rounded(px(2.)).bg(rgb(line)))),
				)
				.when(choice == ThemeChoice::System, |d| d.child(div().absolute().right_0().top_0().bottom_0().w(relative(0.5)).bg(rgb(theme::DARK.background))))
				.relative();
			div()
				.id(match choice {
					ThemeChoice::System => "settings-theme-system",
					ThemeChoice::Light => "settings-theme-light",
					ThemeChoice::Dark => "settings-theme-dark",
				})
				.w(px(168.))
				.p_2()
				.flex()
				.flex_col()
				.gap_2()
				.rounded(px(8.))
				.border_1()
				.cursor_pointer()
				.border_color(rgb(if selected { p.primary } else { p.border }))
				.bg(rgb(p.background))
				.when(!selected, |d| d.hover(|s| s.border_color(rgb(p.muted_foreground))))
				.child(preview)
				.child(
					div()
						.flex()
						.items_center()
						.gap_1p5()
						.text_sm()
						.when(selected, |d| d.child(div().size(px(6.)).rounded_full().bg(rgb(p.primary))))
						.child(choice.label()),
				)
				.on_click(cx.listener(move |_, _, window, cx| {
					theme::set_choice(choice, window, cx);
					cx.notify();
				}))
				.test_support()
		};
		div()
			.flex()
			.flex_col()
			.gap_4()
			.child(Self::section("配色方案", "跟随系统时随 macOS 外观自动切换浅色 / 深色。选择即时生效，并在下次启动时保留。", p))
			.child(div().flex().gap_3().children(ThemeChoice::ALL.map(|choice| card(choice, cx))))
	}

	fn render_packages(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let busy = self.job.is_some();
		let has_input = !self.install.read(cx).value().trim().is_empty();
		let rows: Vec<AnyElement> = match &self.packages {
			None => Vec::new(),
			Some(list) if list.is_empty() => vec![
				div()
					.p_4()
					.text_sm()
					.text_color(rgb(p.muted_foreground))
					.child("还没有安装任何包。在上面输入来源安装，例如 npm:@upstash/context7-omp。")
					.into_any_element(),
			],
			Some(list) => list
				.iter()
				.enumerate()
				.map(|(index, package)| {
					let confirming = self.confirm.as_deref() == Some(package.source.as_str());
					let (update_source, confirm_source, removed) = (package.source.clone(), package.source.clone(), package.clone());
					let detail = package.description().or_else(|| package.path.as_ref().map(|path| display_path(path)));
					div()
						.id(("package", index))
						.flex()
						.items_center()
						.gap_3()
						.px_3()
						.py_2()
						.border_b_1()
						.border_color(rgb(p.border))
						.hover(|s| s.bg(rgb(p.background)))
						.child(
							div()
								.size(px(28.))
								.flex_none()
								.flex()
								.items_center()
								.justify_center()
								.rounded(px(6.))
								.bg(rgb(p.accent))
								.text_color(rgb(p.accent_foreground))
								.text_xs()
								.font_family("Menlo")
								.child(package.kind()),
						)
						.child(
							div()
								.flex_1()
								.min_w_0()
								.flex()
								.flex_col()
								.child(
									div()
										.flex()
										.items_center()
										.gap_2()
										.child(div().min_w_0().truncate().text_sm().font_family("Menlo").child(package.name().to_owned()))
										.children(package.version().map(|v| div().flex_none().text_xs().text_color(rgb(p.muted_foreground)).child(format!("v{v}"))))
										.when(package.scope == Scope::Project, |d| d.child(tag("项目", p)))
										.when(package.filtered, |d| d.child(tag("已筛选", p)))
										.when(package.path.is_none(), |d| {
											d.child(div().flex_none().text_xs().text_color(rgb(p.warning)).child("未安装"))
										}),
								)
								.children(detail.map(|d| div().truncate().text_xs().text_color(rgb(p.muted_foreground)).child(d))),
						)
						.when(!confirming, |r| {
							r.child(
								Button::new(("package-update", index))
									.xsmall()
									.ghost()
									.label("更新")
									.disabled(busy)
									.on_click(cx.listener(move |this, _, _, cx| this.update(Some(update_source.clone()), cx))),
							)
							.child(
								Button::new(("package-remove", index))
									.xsmall()
									.ghost()
									.label("移除")
									.disabled(busy)
									.on_click(cx.listener(move |this, _, _, cx| {
										this.confirm = Some(confirm_source.clone());
										cx.notify();
									})),
							)
						})
						.when(confirming, |r| {
							r.child(div().text_xs().text_color(rgb(p.danger)).child("确认移除？"))
								.child(
									Button::new(("package-confirm", index))
										.xsmall()
										.danger()
										.label("移除")
										.on_click(cx.listener(move |this, _, _, cx| this.remove(removed.clone(), cx))),
								)
								.child(Button::new(("package-keep", index)).xsmall().ghost().label("保留").on_click(cx.listener(|this, _, _, cx| {
									this.confirm = None;
									cx.notify();
								})))
						})
						.test_support()
						.into_any_element()
				})
				.collect(),
		};
		let count = self.packages.as_ref().map_or(String::new(), |list| format!("{} 个", list.len()));
		div()
			.flex()
			.flex_col()
			.gap_3()
			.flex_1()
			.min_h_0()
			.child(Self::section(
				"omp 包",
				"通过 omp install 安装的扩展、技能、提示词与主题，记录在 ~/.omp/agent/settings.json。改动对新开的会话生效。",
				p,
			))
			.child(
				div()
					.flex()
					.gap_2()
					.child(div().id("settings-install-input").flex_1().child(Input::new(&self.install).small()).test_support())
					.child(
						Button::new("settings-install")
							.small()
							.primary()
							.label("安装")
							.disabled(busy || !has_input)
							.on_click(cx.listener(|this, _, window, cx| this.install(window, cx))),
					),
			)
			.child(
				div()
					.flex()
					.items_center()
					.gap_2()
					.child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(rgb(p.muted_foreground)).child(format!("已安装 {count}")))
					.child(div().flex_1())
					.children(self.job.as_ref().map(|job| div().text_xs().text_color(rgb(p.muted_foreground)).child(job.label())))
					.child(Button::new("settings-packages-refresh").xsmall().ghost().label("刷新").disabled(busy).on_click(cx.listener(|this, _, _, cx| this.refresh(cx))))
					.child(
						Button::new("settings-packages-update-all")
							.xsmall()
							.outline()
							.label("全部更新")
							.disabled(busy || self.packages.as_ref().is_none_or(Vec::is_empty))
							.on_click(cx.listener(|this, _, _, cx| this.update(None, cx))),
					),
			)
			.child(
				div()
					.id("package-list")
					.flex_1()
					.min_h_0()
					.overflow_y_scroll()
					.rounded(px(6.))
					.border_1()
					.border_color(rgb(p.border))
					.bg(rgb(p.elevated))
					.children(rows),
			)
	}
}

fn tag(text: &'static str, p: &theme::Palette) -> Div {
	div().flex_none().px_1p5().rounded(px(3.)).bg(rgb(p.muted)).text_xs().text_color(rgb(p.muted_foreground)).child(text)
}

impl Render for SettingsView {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let body = match self.page {
			Page::General => self.render_general(cx).into_any_element(),
			Page::Appearance => self.render_appearance(cx).into_any_element(),
			Page::Packages => self.render_packages(cx).into_any_element(),
		};
		let notice = self.notice.clone().map(|notice| {
			let (color, text) = match notice {
				Notice::Info(text) => (p.accent_foreground, text),
				Notice::Error(text) => (p.danger, text),
			};
			div().id("settings-notice").text_xs().text_color(rgb(color)).child(text).test_support()
		});
		div()
			.id("settings")
			.w(px(800.))
			.h(px(560.))
			.flex()
			.rounded(px(10.))
			.overflow_hidden()
			.border_1()
			.border_color(rgb(p.border))
			.bg(rgb(p.elevated))
			.text_color(cx.theme().foreground)
			.shadow_lg()
			// Clicks inside must not reach the overlay, which closes the panel.
			.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
			.child(self.render_nav(cx))
			.child(
				div()
					.flex_1()
					.min_w_0()
					.flex()
					.flex_col()
					.child(
						div()
							.h(px(40.))
							.flex_none()
							.flex()
							.items_center()
							.px_4()
							.border_b_1()
							.border_color(rgb(p.border))
							.child(div().flex_1().text_sm().font_weight(FontWeight::MEDIUM).child(self.page.label()))
							.child(Button::new("settings-close").ghost().small().label("关闭").on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close)))),
					)
					.child(div().id("settings-body").flex_1().min_h_0().flex().flex_col().gap_3().p_4().overflow_y_scroll().child(body).children(notice)),
			)
	}
}

pub fn display_path(path: &Path) -> String {
	if let Some(home) = launch_env::home_dir()
		&& let Ok(rest) = path.strip_prefix(&home)
	{
		return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
	}
	path.display().to_string()
}
