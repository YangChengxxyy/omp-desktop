//! Slash-command completion for the composer, from omp's
//! `available_commands_update` catalog. Free of GPUI so it can be tested
//! directly.

use omp_rpc::{AvailableSlashCommand, SlashCommandSource};

/// One completion row.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
	/// Text that replaces the composer value when accepted (`/name ` or
	/// `/name sub `).
	pub insert:      String,
	/// `/name` or `/name sub`, as shown.
	pub label:       String,
	/// The alias that matched, when it was not the name.
	pub alias:       Option<String>,
	pub description: Option<String>,
	/// Argument hint or subcommand usage.
	pub hint:        Option<String>,
	pub source:      SlashCommandSource,
}

pub fn source_label(source: SlashCommandSource) -> &'static str {
	match source {
		SlashCommandSource::Builtin => "内置",
		SlashCommandSource::Skill => "skill",
		SlashCommandSource::Extension => "扩展",
		SlashCommandSource::Custom => "自定义",
		SlashCommandSource::McpPrompt => "MCP",
		SlashCommandSource::File => "文件",
	}
}

/// Most rows the popup shows.
pub const LIMIT: usize = 50;

/// Completions for the composer `text`, or `None` when the text is not a
/// slash command being typed: it must start with `/`, be a single line, and
/// the cursor sits in the command word or, after a known command with
/// subcommands, in the subcommand word.
pub fn suggest(commands: &[AvailableSlashCommand], text: &str) -> Option<Vec<Suggestion>> {
	let rest = text.strip_prefix('/')?;
	if rest.contains('\n') {
		return None;
	}
	match rest.split_once(' ') {
		None => Some(command_matches(commands, rest)),
		Some((name, args)) => {
			// Only the subcommand word completes; once it is followed by a
			// space the user is typing free arguments.
			if args.contains(' ') {
				return None;
			}
			let command = find(commands, name)?;
			let subs = command.subcommands.as_deref().filter(|s| !s.is_empty())?;
			let needle = args.to_lowercase();
			let rows: Vec<_> = subs
				.iter()
				.filter(|sub| sub.name.to_lowercase().starts_with(&needle))
				.map(|sub| Suggestion {
					insert:      format!("/{} {} ", command.name, sub.name),
					label:       format!("/{} {}", command.name, sub.name),
					alias:       None,
					description: sub.description.clone(),
					hint:        sub.usage.clone(),
					source:      command.source,
				})
				.take(LIMIT)
				.collect();
			// Nothing to offer once the word is complete and exact.
			if rows.len() == 1 && rows[0].insert.trim_end() == text.trim_end() {
				return Some(Vec::new());
			}
			Some(rows)
		},
	}
}

/// The command `name` (or one of its aliases), case-insensitively.
pub fn find<'a>(commands: &'a [AvailableSlashCommand], name: &str) -> Option<&'a AvailableSlashCommand> {
	let name = name.to_lowercase();
	commands.iter().find(|c| {
		c.name.to_lowercase() == name || c.aliases.iter().flatten().any(|alias| alias.to_lowercase() == name)
	})
}

/// Name prefix matches first (shorter names first), then alias prefixes,
/// then names containing the text.
fn command_matches(commands: &[AvailableSlashCommand], typed: &str) -> Vec<Suggestion> {
	let needle = typed.to_lowercase();
	let mut ranked: Vec<(u8, usize, &AvailableSlashCommand, Option<String>)> = Vec::new();
	for command in commands {
		let name = command.name.to_lowercase();
		let alias = command.aliases.iter().flatten().find(|a| a.to_lowercase().starts_with(&needle)).cloned();
		let rank = if name.starts_with(&needle) {
			0
		} else if alias.is_some() {
			1
		} else if name.contains(&needle) {
			2
		} else {
			continue;
		};
		ranked.push((rank, command.name.len(), command, alias.filter(|_| rank == 1)));
	}
	ranked.sort_by(|a, b| (a.0, a.1, &a.2.name).cmp(&(b.0, b.1, &b.2.name)));
	ranked
		.into_iter()
		.take(LIMIT)
		.map(|(_, _, command, alias)| Suggestion {
			insert: format!("/{} ", command.name),
			label: format!("/{}", command.name),
			alias,
			description: command.description.clone(),
			hint: command
				.input
				.as_ref()
				.and_then(|i| i.hint.clone())
				.or_else(|| {
					let subs = command.subcommands.as_deref()?;
					(!subs.is_empty()).then(|| format!("<{}>", subs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join("|")))
				}),
			source: command.source,
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	fn catalog() -> Vec<AvailableSlashCommand> {
		serde_json::from_value(json!([
			{"name":"model","aliases":["models"],"description":"Show current model selection","source":"builtin"},
			{"name":"fast","description":"Toggle fast mode","input":{"hint":"[on|off|status]"},
				"subcommands":[{"name":"on","description":"Enable"},{"name":"off","description":"Disable"},{"name":"status"}],
				"source":"builtin"},
			{"name":"compact","description":"Compact the context","source":"builtin"},
			{"name":"commit","description":"Write a commit","source":"skill"},
			{"name":"fork","source":"builtin"},
			{"name":"export","aliases":["dump"],"input":{"hint":"[path]"},"source":"builtin"},
		]))
		.unwrap()
	}

	fn labels(rows: &[Suggestion]) -> Vec<&str> {
		rows.iter().map(|r| r.label.as_str()).collect()
	}

	#[test]
	fn only_a_leading_slash_on_one_line_completes() {
		let c = catalog();
		assert!(suggest(&c, "").is_none());
		assert!(suggest(&c, "hello /model").is_none());
		assert!(suggest(&c, "/model\nmore").is_none());
		assert_eq!(suggest(&c, "/").unwrap().len(), c.len());
	}

	#[test]
	fn prefix_beats_alias_beats_substring() {
		let c = catalog();
		assert_eq!(labels(&suggest(&c, "/co").unwrap()), ["/commit", "/compact"]);
		let dump = suggest(&c, "/du").unwrap();
		assert_eq!(labels(&dump), ["/export"]);
		assert_eq!(dump[0].alias.as_deref(), Some("dump"));
		// "or" is inside "fork" and "export"; shorter names first.
		assert_eq!(labels(&suggest(&c, "/or").unwrap()), ["/fork", "/export"]);
		assert_eq!(labels(&suggest(&c, "/MOD").unwrap()), ["/model"]);
		assert!(suggest(&c, "/zzz").unwrap().is_empty());
	}

	#[test]
	fn hints_come_from_input_or_subcommands() {
		let c = catalog();
		let fast = &suggest(&c, "/fa").unwrap()[0];
		assert_eq!(fast.hint.as_deref(), Some("[on|off|status]"));
		assert_eq!(fast.insert, "/fast ");
		let model = &suggest(&c, "/model").unwrap()[0];
		assert_eq!(model.hint, None);
	}

	#[test]
	fn subcommands_complete_after_the_command() {
		let c = catalog();
		assert_eq!(labels(&suggest(&c, "/fast ").unwrap()), ["/fast on", "/fast off", "/fast status"]);
		let off = suggest(&c, "/fast of").unwrap();
		assert_eq!(labels(&off), ["/fast off"]);
		assert_eq!(off[0].insert, "/fast off ");
		assert!(suggest(&c, "/fast off").unwrap().is_empty(), "exact subcommand needs no popup");
		assert!(suggest(&c, "/fast off now").is_none(), "free arguments");
		assert!(suggest(&c, "/model ").is_none(), "no subcommands");
		assert!(suggest(&c, "/nope x").is_none(), "unknown command");
		// Aliases resolve to their command.
		assert!(suggest(&c, "/dump ").is_none());
	}
}
