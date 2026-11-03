//! Accounts panel: lists omp's login providers, runs OAuth login, and
//! removes stored credentials.
//!
//! It drives its own omp process, so a login never waits behind a session
//! turn and works before any session can start: omp refuses to start
//! without a usable model, so this process gets a placeholder API key in
//! its environment. The key only lets omp boot; it is never stored and this
//! process makes no model calls. Cancelling a login restarts the process,
//! because omp re-asks a cancelled code prompt at once (see `login.rs`).

use std::sync::Arc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use omp_bridge::{Session, SessionOptions};
use omp_rpc::{
	Client, Event, GetLoginProvidersCommand, GetLogoutAccountsCommand, LoginCommand, LoginProvider, LogoutAccount,
	LogoutCommand, RpcInbound, RpcNotification,
};

use crate::login::{self, FlowAction, LoginFlow};
use crate::theme;

/// Lets omp start with no stored credentials (see the module docs).
const PLACEHOLDER_KEY: (&str, &str) = ("OPENAI_API_KEY", "omp-desktop-accounts-placeholder");

pub enum AccountsEvent {
	/// Credentials changed (login or logout succeeded).
	Changed,
	Close,
}

impl EventEmitter<AccountsEvent> for AccountsView {}

enum Conn {
	Connecting,
	Connected(Arc<Client>),
	Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
	Info(String),
	Error(String),
}

pub struct AccountsView {
	options:   SessionOptions,
	conn:      Conn,
	providers: Vec<LoginProvider>,
	search:    Entity<InputState>,
	code:      Entity<InputState>,
	/// Login in progress; `generation` tells a stale `login` reply (from a
	/// process restarted by cancel) from the current one.
	flow:      Option<LoginFlow>,
	generation: u64,
	/// Process running the login call; that call holds the client for up to
	/// ten minutes, so cancelling (or closing the panel) kills the process.
	login_pid: Option<u32>,
	/// Provider whose stored accounts are listed, with the accounts.
	accounts:  Option<(String, Vec<LogoutAccount>)>,
	/// Account waiting for the user to confirm its removal.
	confirm:   Option<LogoutAccount>,
	/// Provider that has to log in from a terminal.
	terminal:  Option<String>,
	message:   Option<Message>,
	busy:      bool,
	_tasks:    Vec<Task<()>>,
	_subs:     Vec<Subscription>,
}

impl AccountsView {
	pub fn new(base: &SessionOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let mut env = base.env.clone();
		env.push((PLACEHOLDER_KEY.0.to_owned(), PLACEHOLDER_KEY.1.to_owned()));
		let options = SessionOptions {
			omp: base.omp.clone(),
			cwd: Some(std::env::temp_dir()),
			resume: None,
			session_dir: None,
			extra_args: ["--no-session", "--no-tools", "--no-lsp", "--no-title", "--thinking", "off"]
				.map(String::from)
				.to_vec(),
			env,
		};
		let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索 provider"));
		search.update(cx, |state, cx| state.focus(window, cx));
		let code = cx.new(|cx| InputState::new(window, cx).placeholder("授权码或回调 URL"));
		let subs = vec![
			cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
				if matches!(event, InputEvent::Change) {
					cx.notify();
				}
			}),
			cx.subscribe(&code, |this, _, event: &InputEvent, cx| {
				if matches!(event, InputEvent::PressEnter { .. }) {
					this.submit_code(cx);
				}
			}),
		];
		let mut view = Self {
			options,
			conn: Conn::Connecting,
			providers: Vec::new(),
			search,
			code,
			flow: None,
			generation: 0,
			login_pid: None,
			accounts: None,
			confirm: None,
			terminal: None,
			message: None,
			busy: false,
			_tasks: Vec::new(),
			_subs: subs,
		};
		view.connect(cx);
		view
	}

	fn client(&self) -> Option<Arc<Client>> {
		match &self.conn {
			Conn::Connected(client) => Some(client.clone()),
			_ => None,
		}
	}

	fn connect(&mut self, cx: &mut Context<Self>) {
		self.conn = Conn::Connecting;
		self.generation += 1;
		let generation = self.generation;
		let options = self.options.clone();
		let spawn = cx.background_spawn(async move {
			let session = Session::spawn(&options)?;
			let providers = session.client.call(&GetLoginProvidersCommand {})?;
			Ok::<_, omp_bridge::BridgeError>((session, providers))
		});
		let task = cx.spawn(async move |this, cx| {
			let (session, providers) = match spawn.await {
				Ok(started) => started,
				Err(error) => {
					_ = this.update(cx, |this, cx| {
						this.conn = Conn::Failed(error.to_string());
						cx.notify();
					});
					return;
				},
			};
			let events = session.events.clone();
			_ = this.update(cx, |this, cx| {
				this.conn = Conn::Connected(session.client.clone());
				this.providers = providers;
				cx.notify();
			});
			while let Ok(event) = events.recv().await {
				let Event::Notification(RpcNotification::ExtensionUiRequest(request)) = event else {
					continue;
				};
				let alive = this.update(cx, |this, cx| this.on_ui_request(generation, request, cx));
				if alive.is_err() {
					return;
				}
			}
			_ = this.update(cx, |this, cx| {
				if this.generation == generation {
					this.conn = Conn::Failed("omp 进程已退出".into());
					this.flow = None;
					cx.notify();
				}
			});
		});
		// Dropping the old task drops the old client, which stops its process.
		self._tasks = vec![task];
	}

	fn on_ui_request(&mut self, generation: u64, request: omp_rpc::ExtensionUiRequest, cx: &mut Context<Self>) {
		if generation != self.generation {
			return;
		}
		let Some(flow) = &mut self.flow else {
			// Outside a login nothing here asks the user anything; decline so
			// omp does not wait for an answer.
			if let Some(id) = crate::session::dialog_id(&request)
				&& let Some(client) = self.client()
			{
				_ = client.send(&RpcInbound::ExtensionUiResponse(login::decline(id)));
			}
			return;
		};
		match flow.apply(request) {
			FlowAction::None => {},
			FlowAction::OpenBrowser(url) => cx.open_url(&url),
			FlowAction::Reply(response) => {
				if let Some(client) = self.client() {
					_ = client.send(&RpcInbound::ExtensionUiResponse(response));
				}
			},
		}
		cx.notify();
	}

	fn refresh(&mut self, cx: &mut Context<Self>) {
		let Some(client) = self.client() else {
			return;
		};
		let fetch = cx.background_spawn(async move { client.call(&GetLoginProvidersCommand {}) });
		cx.spawn(async move |this, cx| {
			let providers = fetch.await;
			_ = this.update(cx, |this, cx| {
				match providers {
					Ok(providers) => this.providers = providers,
					Err(error) => this.message = Some(Message::Error(format!("读取 provider 失败：{error}"))),
				}
				cx.notify();
			});
		})
		.detach();
	}

	pub fn login(&mut self, provider_id: String, cx: &mut Context<Self>) {
		let Some(client) = self.client() else {
			return;
		};
		if self.flow.is_some() || self.busy {
			return;
		}
		self.message = None;
		self.terminal = None;
		self.confirm = None;
		self.flow = Some(LoginFlow::new(provider_id.clone()));
		self.login_pid = client.pid();
		let generation = self.generation;
		let id = provider_id.clone();
		// The call blocks until the browser flow ends (up to 10 minutes): run
		// it on its own thread rather than an executor worker.
		let (tx, rx) = async_channel::bounded(1);
		std::thread::Builder::new()
			.name("omp-login".into())
			.spawn(move || _ = tx.send_blocking(client.call(&LoginCommand { provider_id: id })))
			.expect("spawn login thread");
		cx.spawn(async move |this, cx| {
			let Ok(result) = rx.recv().await else {
				return;
			};
			_ = this.update(cx, |this, cx| {
				if this.generation != generation {
					return;
				}
				this.flow = None;
				this.login_pid = None;
				match result {
					Ok(_) => {
						let name = this.provider_name(&provider_id);
						this.message = Some(Message::Info(format!("已登录 {name}")));
						cx.emit(AccountsEvent::Changed);
					},
					Err(omp_rpc::Error::Command { error, .. }) if login::needs_terminal(&error) => {
						this.terminal = Some(provider_id.clone());
					},
					Err(error) => this.message = Some(Message::Error(format!("登录失败：{error}"))),
				}
				this.refresh(cx);
				cx.notify();
			});
		})
		.detach();
		cx.notify();
	}

	/// Stops the login by restarting the process (see the module docs).
	pub fn cancel_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		if self.flow.take().is_none() {
			return;
		}
		self.message = Some(Message::Info("已取消登录".into()));
		self.code.update(cx, |state, cx| state.set_value("", window, cx));
		if let Some(pid) = self.login_pid.take() {
			omp_bridge::terminate_group(pid);
		}
		self.connect(cx);
		cx.notify();
	}

	fn submit_code(&mut self, cx: &mut Context<Self>) {
		let code = self.code.read(cx).value().to_string();
		let Some(response) = self.flow.as_mut().and_then(|flow| flow.submit_code(&code)) else {
			return;
		};
		if let Some(client) = self.client() {
			_ = client.send(&RpcInbound::ExtensionUiResponse(response));
		}
		cx.notify();
	}

	fn show_accounts(&mut self, provider_id: String, cx: &mut Context<Self>) {
		if self.accounts.as_ref().is_some_and(|(id, _)| *id == provider_id) {
			self.accounts = None;
			self.confirm = None;
			cx.notify();
			return;
		}
		let Some(client) = self.client() else {
			return;
		};
		let id = provider_id.clone();
		let fetch = cx.background_spawn(async move { client.call(&GetLogoutAccountsCommand { provider_id: id }) });
		cx.spawn(async move |this, cx| {
			let accounts = fetch.await;
			_ = this.update(cx, |this, cx| {
				match accounts {
					Ok(accounts) => {
						if accounts.is_empty() {
							this.message =
								Some(Message::Info("没有存储的凭据；当前认证来自环境变量或配置，需在源头移除".into()));
						}
						this.accounts = Some((provider_id, accounts));
					},
					Err(error) => this.message = Some(Message::Error(format!("读取账号失败：{error}"))),
				}
				this.confirm = None;
				cx.notify();
			});
		})
		.detach();
	}

	fn logout(&mut self, account: LogoutAccount, cx: &mut Context<Self>) {
		let Some(client) = self.client() else {
			return;
		};
		self.busy = true;
		self.confirm = None;
		let label = format!("{} · {}", self.provider_name(&account.provider), account.label);
		let call = cx.background_spawn(async move {
			client.call(&LogoutCommand { provider_id: account.provider.clone(), credential_id: account.credential_id })
		});
		cx.spawn(async move |this, cx| {
			let result = call.await;
			_ = this.update(cx, |this, cx| {
				this.busy = false;
				this.accounts = None;
				match result {
					Ok(result) => {
						let tail = result.remaining_source.map(|s| format!("；仍通过 {s} 认证")).unwrap_or_default();
						this.message = Some(Message::Info(format!("已移除 {label}{tail}")));
						cx.emit(AccountsEvent::Changed);
					},
					Err(error) => this.message = Some(Message::Error(format!("退出登录失败：{error}"))),
				}
				this.refresh(cx);
				cx.notify();
			});
		})
		.detach();
		cx.notify();
	}

	fn provider_name(&self, id: &str) -> String {
		self.providers.iter().find(|p| p.id == id).map_or_else(|| id.to_owned(), |p| p.name.clone())
	}

	#[cfg(test)]
	pub(crate) fn login_pid(&self) -> Option<u32> {
		self.login_pid
	}

	#[cfg(test)]
	pub(crate) fn connected(&self) -> bool {
		matches!(self.conn, Conn::Connected(_))
	}

	#[cfg(test)]
	pub(crate) fn providers(&self) -> &[LoginProvider] {
		&self.providers
	}

	#[cfg(test)]
	pub(crate) fn flow(&self) -> Option<&LoginFlow> {
		self.flow.as_ref()
	}

	#[cfg(test)]
	pub(crate) fn message(&self) -> Option<&Message> {
		self.message.as_ref()
	}

	#[cfg(test)]
	pub(crate) fn listed_accounts(&self) -> Option<&[LogoutAccount]> {
		self.accounts.as_ref().map(|(_, accounts)| accounts.as_slice())
	}

	fn render_flow(&self, flow: &LoginFlow, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let name = self.provider_name(&flow.provider_id);
		let copy = flow.copy_url.clone();
		let open = flow.url.clone();
		div()
			.id("login-flow")
			.flex()
			.flex_col()
			.gap_2()
			.p_3()
			.rounded(px(8.))
			.border_1()
			.border_color(rgb(p.primary))
			.bg(rgb(p.accent))
			.text_sm()
			.child(div().font_weight(FontWeight::SEMIBOLD).child(format!("正在登录 {name}")))
			.child(
				div().text_xs().text_color(rgb(p.muted_foreground)).child(
					flow.instructions.clone().or_else(|| flow.progress.clone()).unwrap_or_else(|| "等待 omp…".into()),
				),
			)
			.when(flow.url.is_some(), |d| {
				d.child(
					div()
						.flex()
						.gap_2()
						.child(Button::new("login-open").small().primary().label("在浏览器中打开").on_click(cx.listener(
							move |_, _, _, cx| {
								if let Some(url) = &open {
									cx.open_url(url);
								}
							},
						)))
						.child(Button::new("login-copy").small().outline().label("复制链接").on_click(cx.listener(
							move |this, _, _, cx| {
								if let Some(url) = &copy {
									cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
									this.message = Some(Message::Info("链接已复制".into()));
									cx.notify();
								}
							},
						))),
				)
			})
			.when_some(flow.code_prompt.clone(), |d, prompt| {
				d.child(div().text_xs().text_color(rgb(p.muted_foreground)).child(format!(
					"浏览器无法回到本机时：{}",
					prompt.title
				)))
				.child(
					div()
						.flex()
						.gap_2()
						.child(div().id("login-code").flex_1().child(Input::new(&self.code).small()).test_support())
						.child(
							Button::new("login-submit")
								.small()
								.label("提交")
								.on_click(cx.listener(|this, _, _, cx| this.submit_code(cx))),
						),
				)
			})
			.child(
				div().flex().justify_end().child(
					Button::new("login-cancel")
						.small()
						.ghost()
						.label("取消登录")
						.on_click(cx.listener(|this, _, window, cx| this.cancel_login(window, cx))),
				),
			)
			.test_support()
	}

	fn render_accounts(&self, provider: &str, accounts: &[LogoutAccount], cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let rows = accounts.iter().enumerate().map(|(index, account)| {
			let confirming = self.confirm.as_ref() == Some(account);
			let pick = account.clone();
			let confirm = account.clone();
			div()
				.id(("account", index))
				.flex()
				.items_center()
				.gap_2()
				.px_2()
				.py_1()
				.text_xs()
				.child(div().flex_none().font_family("Menlo").child(login::account_kind(account)))
				.child(div().flex_1().min_w_0().truncate().child(format!("{} · {}", account.label, account.detail)))
				.when(account.active, |r| r.child(div().text_color(rgb(p.success)).child("当前使用")))
				.when(!confirming, |r| {
					r.child(
						Button::new(("account-logout", index))
							.xsmall()
							.outline()
							.label("退出登录")
							.disabled(self.busy)
							.on_click(cx.listener(move |this, _, _, cx| {
								this.confirm = Some(pick.clone());
								cx.notify();
							})),
					)
				})
				.when(confirming, |r| {
					r.child(div().text_color(rgb(p.danger)).child("确认移除？"))
						.child(
							Button::new(("account-confirm", index))
								.xsmall()
								.danger()
								.label("移除")
								.on_click(cx.listener(move |this, _, _, cx| this.logout(confirm.clone(), cx))),
						)
						.child(Button::new(("account-keep", index)).xsmall().ghost().label("保留").on_click(cx.listener(
							|this, _, _, cx| {
								this.confirm = None;
								cx.notify();
							},
						)))
				})
				.test_support()
		});
		div()
			.id(SharedString::from(format!("accounts-{provider}")))
			.ml_4()
			.mb_1()
			.rounded(px(6.))
			.border_1()
			.border_color(rgb(p.border))
			.bg(rgb(p.panel))
			.children(rows)
	}
}

impl Drop for AccountsView {
	fn drop(&mut self) {
		if let Some(pid) = self.login_pid.take() {
			omp_bridge::terminate_group(pid);
		}
	}
}

impl Render for AccountsView {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let p = theme::palette(cx);
		let query = self.search.read(cx).value().to_string();
		let connected = matches!(self.conn, Conn::Connected(_));
		let logging_in = self.flow.is_some();
		let rows: Vec<AnyElement> = login::filter(&self.providers, &query, false)
			.into_iter()
			.enumerate()
			.flat_map(|(index, provider)| {
				let (login_id, accounts_id) = (provider.id.clone(), provider.id.clone());
				let row = div()
					.id(("provider", index))
					.flex()
					.items_center()
					.gap_2()
					.px_2()
					.h(px(30.))
					.rounded(px(6.))
					.text_sm()
					.hover(|s| s.bg(rgb(p.muted)))
					.child(div().flex_1().min_w_0().truncate().child(provider.name.clone()))
					.child(div().flex_none().text_xs().font_family("Menlo").text_color(rgb(p.muted_foreground)).child(provider.id.clone()))
					.when(provider.authenticated, |r| {
						r.child(div().flex_none().text_xs().text_color(rgb(p.success)).child("● 已登录")).child(
							Button::new(("provider-accounts", index)).xsmall().ghost().label("账号").on_click(cx.listener(
								move |this, _, _, cx| this.show_accounts(accounts_id.clone(), cx),
							)),
						)
					})
					.child(
						Button::new(("provider-login", index))
							.xsmall()
							.outline()
							.label(if provider.authenticated { "再登录" } else { "登录" })
							.disabled(!connected || logging_in || !provider.available)
							.on_click(cx.listener(move |this, _, _, cx| this.login(login_id.clone(), cx))),
					)
					.test_support()
					.into_any_element();
				let accounts = self
					.accounts
					.as_ref()
					.filter(|(id, _)| *id == provider.id)
					.map(|(id, accounts)| self.render_accounts(id, accounts, cx).into_any_element());
				std::iter::once(row).chain(accounts)
			})
			.collect();

		let status: Option<AnyElement> = match &self.conn {
			Conn::Connecting => Some(div().text_sm().text_color(rgb(p.muted_foreground)).child("正在启动 omp…").into_any_element()),
			Conn::Failed(error) => Some(
				div()
					.flex()
					.items_center()
					.gap_2()
					.text_sm()
					.text_color(rgb(p.danger))
					.child(div().flex_1().child(format!("无法启动 omp：{error}")))
					.child(Button::new("accounts-retry").small().label("重试").on_click(cx.listener(|this, _, _, cx| this.connect(cx))))
					.into_any_element(),
			),
			Conn::Connected(_) => None,
		};

		div()
			.id("accounts")
			.w(px(620.))
			.h(px(600.))
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
			.child(
				div()
					.flex()
					.items_center()
					.child(div().flex_1().text_base().font_weight(FontWeight::SEMIBOLD).child("账号与登录"))
					.child(
						Button::new("accounts-close")
							.ghost()
							.small()
							.label("关闭")
							.on_click(cx.listener(|_, _, _, cx| cx.emit(AccountsEvent::Close))),
					),
			)
			.child(
				div()
					.text_xs()
					.text_color(rgb(p.muted_foreground))
					.child("凭据保存在 omp 的凭据库（~/.omp/agent），所有会话共用；新开的会话使用新凭据。"),
			)
			.children(status)
			.when_some(self.flow.clone(), |d, flow| d.child(self.render_flow(&flow, cx)))
			.when_some(self.terminal.clone(), |d, provider| {
				let command = login::terminal_command(&provider);
				d.child(
					div()
						.id("login-terminal")
						.flex()
						.flex_col()
						.gap_2()
						.p_3()
						.rounded(px(8.))
						.border_1()
						.border_color(rgb(p.warning))
						.text_sm()
						.child(format!("{} 需要输入密钥，客户端里无法完成。请在终端运行：", self.provider_name(&provider)))
						.child(
							div()
								.flex()
								.gap_2()
								.items_center()
								.child(
									div()
										.flex_1()
										.font_family("Menlo")
										.text_xs()
										.px_2()
										.py_1()
										.rounded(px(6.))
										.bg(rgb(p.code))
										.child(command.clone()),
								)
								.child(Button::new("terminal-copy").small().outline().label("复制命令").on_click(cx.listener(
									move |this, _, _, cx| {
										cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
										this.message = Some(Message::Info("命令已复制；登录后点“刷新”".into()));
										cx.notify();
									},
								))),
						)
						.test_support(),
				)
			})
			.when_some(self.message.clone(), |d, message| {
				let (color, text) = match message {
					Message::Info(text) => (p.accent_foreground, text),
					Message::Error(text) => (p.danger, text),
				};
				d.child(div().id("accounts-message").text_xs().text_color(rgb(color)).child(text).test_support())
			})
			.child(
				div()
					.flex()
					.gap_2()
					.child(div().id("accounts-search").flex_1().child(Input::new(&self.search).small()).test_support())
					.child(
						Button::new("accounts-refresh")
							.small()
							.ghost()
							.label("刷新")
							.disabled(!connected)
							.on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
					),
			)
			.child(div().id("provider-list").flex_1().min_h_0().overflow_y_scroll().children(rows))
	}
}
