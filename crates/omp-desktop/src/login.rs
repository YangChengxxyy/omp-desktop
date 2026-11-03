//! Provider list and OAuth login state for the accounts panel. Free of GPUI
//! so it can be tested directly.
//!
//! omp drives `login` through UI requests on the same process: `open_url`
//! (the authorize link), `notify` (progress), and one `input` (paste the
//! code when the browser cannot reach omp's loopback callback). Answering
//! that `input` with nothing makes omp ask again at once, so a login is
//! cancelled by stopping its process, never by cancelling the prompt.

use omp_rpc::{
	CancelUiResponse, ExtensionUiRequest, ExtensionUiResponse, LitTrue, LoginProvider, LogoutAccount, LogoutAccountType,
};

/// Providers matching every whitespace-separated term of `query` (in the id
/// or the name, case-insensitively); authenticated ones first, otherwise in
/// omp's order.
pub fn filter<'a>(providers: &'a [LoginProvider], query: &str, only_authenticated: bool) -> Vec<&'a LoginProvider> {
	let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
	let mut rows: Vec<&LoginProvider> = providers
		.iter()
		.filter(|p| !only_authenticated || p.authenticated)
		.filter(|p| {
			let haystack = format!("{} {}", p.id, p.name).to_lowercase();
			terms.iter().all(|term| haystack.contains(term))
		})
		.collect();
	rows.sort_by_key(|p| !p.authenticated);
	rows
}

/// `omp login <id>`, for providers whose login needs the terminal.
pub fn terminal_command(provider_id: &str) -> String {
	format!("omp login {provider_id}")
}

/// The provider asks for a secret or a prompt before the browser step,
/// which RPC mode refuses; the user has to log in from a terminal.
pub fn needs_terminal(error: &str) -> bool {
	error.contains("not supported in RPC mode")
}

pub fn account_kind(account: &LogoutAccount) -> &'static str {
	match account.r#type {
		LogoutAccountType::ApiKey => "API key",
		LogoutAccountType::Oauth => "OAuth",
	}
}

/// The `input` request asking for the authorization code.
#[derive(Debug, Clone, PartialEq)]
pub struct CodePrompt {
	pub id:          String,
	pub title:       String,
	pub placeholder: Option<String>,
}

/// One login in progress.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LoginFlow {
	pub provider_id:  String,
	/// Authorize URL from `open_url`.
	pub url:          Option<String>,
	/// What "copy link" copies: omp's short loopback `launchUrl` when it
	/// sent one (long URLs get truncated by some terminals and chat apps),
	/// otherwise the full URL.
	pub copy_url:     Option<String>,
	pub instructions: Option<String>,
	/// Latest `notify` text ("Waiting for browser authentication...").
	pub progress:     Option<String>,
	pub code_prompt:  Option<CodePrompt>,
}

/// What the view must do for a UI request that arrived during a login.
#[derive(Debug, Clone, PartialEq)]
pub enum FlowAction {
	None,
	OpenBrowser(String),
	Reply(ExtensionUiResponse),
}

impl LoginFlow {
	pub fn new(provider_id: impl Into<String>) -> Self {
		Self { provider_id: provider_id.into(), ..Default::default() }
	}

	pub fn apply(&mut self, request: ExtensionUiRequest) -> FlowAction {
		match request {
			ExtensionUiRequest::OpenUrl(open) => {
				self.copy_url = Some(open.launch_url.clone().unwrap_or_else(|| open.url.clone()));
				self.instructions = open.instructions;
				self.url = Some(open.url.clone());
				FlowAction::OpenBrowser(open.url)
			},
			ExtensionUiRequest::Notify(notify) => {
				self.progress = Some(notify.message);
				FlowAction::None
			},
			ExtensionUiRequest::Input(input) => {
				self.code_prompt = Some(CodePrompt { id: input.id, title: input.title, placeholder: input.placeholder });
				FlowAction::None
			},
			ExtensionUiRequest::Cancel(cancel) => {
				if self.code_prompt.as_ref().is_some_and(|prompt| prompt.id == cancel.target_id) {
					self.code_prompt = None;
				}
				FlowAction::None
			},
			// Login uses none of these; decline so omp does not wait.
			request @ (ExtensionUiRequest::Select(_)
			| ExtensionUiRequest::Confirm(_)
			| ExtensionUiRequest::Editor(_)
			| ExtensionUiRequest::Ask(_)) => match crate::session::dialog_id(&request) {
				Some(id) => FlowAction::Reply(decline(id)),
				None => FlowAction::None,
			},
			_ => FlowAction::None,
		}
	}

	/// The answer to the code prompt; `None` while there is none or the
	/// code is blank (omp would just ask again).
	pub fn submit_code(&mut self, code: &str) -> Option<ExtensionUiResponse> {
		let code = code.trim();
		if code.is_empty() {
			return None;
		}
		let prompt = self.code_prompt.take()?;
		Some(ExtensionUiResponse::ValueUiResponse(omp_rpc::ValueUiResponse { id: prompt.id, value: code.to_owned() }))
	}
}

pub fn decline(id: &str) -> ExtensionUiResponse {
	ExtensionUiResponse::CancelUiResponse(CancelUiResponse { id: id.to_owned(), cancelled: LitTrue, timed_out: None })
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	fn providers() -> Vec<LoginProvider> {
		serde_json::from_value(json!([
			{"id":"openai-codex","name":"ChatGPT Plus/Pro (Codex)","available":true,"authenticated":false},
			{"id":"anthropic","name":"Anthropic (Claude Pro/Max)","available":true,"authenticated":true},
			{"id":"github-copilot","name":"GitHub Copilot","available":true,"authenticated":false},
			{"id":"openrouter","name":"OpenRouter","available":true,"authenticated":true},
		]))
		.unwrap()
	}

	fn request(value: serde_json::Value) -> ExtensionUiRequest {
		ExtensionUiRequest::from_value(value).unwrap()
	}

	#[test]
	fn filter_matches_terms_and_puts_authenticated_first() {
		let list = providers();
		let ids = |rows: Vec<&LoginProvider>| rows.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
		assert_eq!(ids(filter(&list, "", false)), ["anthropic", "openrouter", "openai-codex", "github-copilot"]);
		assert_eq!(ids(filter(&list, "OPEN", false)), ["openrouter", "openai-codex"]);
		assert_eq!(ids(filter(&list, "chatgpt pro", false)), ["openai-codex"]);
		assert_eq!(ids(filter(&list, "", true)), ["anthropic", "openrouter"]);
		assert!(filter(&list, "copilot", true).is_empty());
	}

	#[test]
	fn flow_folds_open_url_progress_and_code_prompt() {
		let mut flow = LoginFlow::new("openai-codex");
		let action = flow.apply(request(json!({
			"type":"extension_ui_request","id":"1","method":"open_url",
			"url":"https://auth.example/authorize?x=1","launchUrl":"http://localhost:1455/launch",
			"instructions":"A browser window should open."
		})));
		assert_eq!(action, FlowAction::OpenBrowser("https://auth.example/authorize?x=1".into()));
		assert_eq!(flow.copy_url.as_deref(), Some("http://localhost:1455/launch"));
		assert_eq!(flow.instructions.as_deref(), Some("A browser window should open."));

		flow.apply(request(json!({"type":"extension_ui_request","id":"2","method":"notify","message":"Waiting..."})));
		assert_eq!(flow.progress.as_deref(), Some("Waiting..."));

		flow.apply(request(json!({"type":"extension_ui_request","id":"3","method":"input","title":"Paste the code:"})));
		assert_eq!(flow.code_prompt.as_ref().map(|p| p.id.as_str()), Some("3"));
		assert_eq!(flow.submit_code("   "), None, "blank code is not sent");
		assert!(flow.code_prompt.is_some());
		let Some(ExtensionUiResponse::ValueUiResponse(answer)) = flow.submit_code(" abc ") else { panic!() };
		assert_eq!((answer.id.as_str(), answer.value.as_str()), ("3", "abc"));
		assert!(flow.code_prompt.is_none());
	}

	#[test]
	fn flow_without_launch_url_copies_the_url_and_declines_other_dialogs() {
		let mut flow = LoginFlow::new("x");
		flow.apply(request(json!({"type":"extension_ui_request","id":"1","method":"open_url","url":"https://a/b"})));
		assert_eq!(flow.copy_url.as_deref(), Some("https://a/b"));
		flow.apply(request(json!({"type":"extension_ui_request","id":"2","method":"input","title":"t"})));
		flow.apply(request(json!({"type":"extension_ui_request","id":"9","method":"cancel","targetId":"2"})));
		assert!(flow.code_prompt.is_none(), "server cancelled the prompt");
		let action = flow.apply(request(json!({"type":"extension_ui_request","id":"4","method":"confirm","title":"t","message":"m"})));
		assert!(matches!(action, FlowAction::Reply(ExtensionUiResponse::CancelUiResponse(c)) if c.id == "4"));
	}

	#[test]
	fn terminal_hint_for_rpc_refusals() {
		assert!(needs_terminal(
			"Provider 'x' requires secret input, which is not supported in RPC mode. Use the terminal UI to log in."
		));
		assert!(!needs_terminal("token exchange failed: 401"));
		assert_eq!(terminal_command("anthropic"), "omp login anthropic");
	}
}
