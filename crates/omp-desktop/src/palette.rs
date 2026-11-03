//! Command palette (⌘K): fuzzy search over the window's actions, the
//! current tab's models and thinking levels, and its slash commands. ⌘M
//! opens it narrowed to models. Free of rendering; `palette_view.rs` draws it.

/// What picking an entry does; the workspace carries it out.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
	NewTab,
	CloseTab,
	SearchSessions,
	ToggleSidebar,
	TogglePanel,
	Accounts,
	Compact,
	CycleThinking,
	Theme(crate::theme::ThemeChoice),
	SetModel { provider: String, id: String },
	SetThinking(omp_rpc::ThinkingLevel),
	/// Puts `/name ` in the composer.
	Slash(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
	Action,
	Model,
	Thinking,
	Slash,
}

impl Group {
	pub fn label(self) -> &'static str {
		match self {
			Self::Action => "操作",
			Self::Model => "模型",
			Self::Thinking => "思考",
			Self::Slash => "命令",
		}
	}
}

#[derive(Clone, Debug)]
pub struct Entry {
	pub group:    Group,
	pub title:    String,
	/// Searched with the title, shown dimmed (provider, description).
	pub detail:   String,
	pub shortcut: Option<&'static str>,
	/// Marks the current model, level, or theme.
	pub current:  bool,
	pub command:  Command,
}

impl Entry {
	pub fn new(group: Group, title: impl Into<String>, command: Command) -> Self {
		Self { group, title: title.into(), detail: String::new(), shortcut: None, current: false, command }
	}

	pub fn detail(mut self, detail: impl Into<String>) -> Self {
		self.detail = detail.into();
		self
	}

	pub fn shortcut(mut self, keys: &'static str) -> Self {
		self.shortcut = Some(keys);
		self
	}

	pub fn current(mut self, current: bool) -> Self {
		self.current = current;
		self
	}
}

/// Entries matching `query`, best first. Every whitespace-separated term
/// must match the title or detail: as a prefix of a word, a substring, or a
/// subsequence of the title, in that order of preference. An empty query
/// keeps the given order.
pub fn rank<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
	let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
	if terms.is_empty() {
		return entries.iter().collect();
	}
	let mut scored: Vec<(u32, usize, &Entry)> = entries
		.iter()
		.enumerate()
		.filter_map(|(index, entry)| {
			let title = entry.title.to_lowercase();
			let detail = entry.detail.to_lowercase();
			let mut total = 0;
			for term in &terms {
				total += score(&title, term).or_else(|| score(&detail, term).map(|s| s + 3))?;
			}
			Some((total, index, entry))
		})
		.collect();
	scored.sort_by_key(|&(score, index, _)| (score, index));
	scored.into_iter().map(|(_, _, entry)| entry).collect()
}

/// Lower is better; `None` when `term` does not match `text`.
fn score(text: &str, term: &str) -> Option<u32> {
	if text.starts_with(term) {
		return Some(0);
	}
	if text.split(|c: char| !c.is_alphanumeric()).any(|word| word.starts_with(term)) {
		return Some(1);
	}
	if text.contains(term) {
		return Some(2);
	}
	let mut chars = text.chars();
	term.chars().all(|t| chars.any(|c| c == t)).then_some(4)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn entries() -> Vec<Entry> {
		vec![
			Entry::new(Group::Action, "新建会话", Command::NewTab).detail("new tab"),
			Entry::new(Group::Action, "压缩上下文", Command::Compact).detail("compact"),
			Entry::new(Group::Model, "claude-sonnet-4-5", Command::SetModel { provider: "anthropic".into(), id: "claude-sonnet-4-5".into() })
				.detail("anthropic"),
			Entry::new(Group::Model, "gpt-5.5", Command::SetModel { provider: "openai".into(), id: "gpt-5.5".into() })
				.detail("openai"),
			Entry::new(Group::Slash, "/compact", Command::Slash("compact".into())).detail("Compact the context"),
		]
	}

	fn titles(entries: &[Entry], query: &str) -> Vec<String> {
		rank(entries, query).into_iter().map(|e| e.title.clone()).collect()
	}

	#[test]
	fn ranks_prefix_word_substring_subsequence() {
		let entries = entries();
		assert_eq!(titles(&entries, "").len(), 5, "empty query keeps everything in order");
		assert_eq!(titles(&entries, "")[0], "新建会话");
		// Title prefix beats a detail match.
		assert_eq!(titles(&entries, "/comp"), ["/compact"]);
		assert_eq!(titles(&entries, "compact"), ["/compact", "压缩上下文"]);
		// Word prefix inside the title.
		assert_eq!(titles(&entries, "sonnet"), ["claude-sonnet-4-5"]);
		// Subsequence.
		assert_eq!(titles(&entries, "cs45"), ["claude-sonnet-4-5"]);
		// Every term must match; detail counts.
		assert_eq!(titles(&entries, "openai gpt"), ["gpt-5.5"]);
		assert!(titles(&entries, "openai claude").is_empty());
		// Case-insensitive.
		assert_eq!(titles(&entries, "GPT"), ["gpt-5.5"]);
	}
}
