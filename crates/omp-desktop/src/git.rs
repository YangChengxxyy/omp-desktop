//! Branch and working-tree state for the status bar, from
//! `git status --porcelain=v2 --branch`. Free of GPUI so it can be tested
//! directly.

use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitStatus {
	/// Branch name, or `None` when HEAD is detached.
	pub branch:     Option<String>,
	/// Abbreviated commit, `None` before the first commit.
	pub oid:        Option<String>,
	pub upstream:   Option<String>,
	pub ahead:      u32,
	pub behind:     u32,
	pub staged:     u32,
	pub unstaged:   u32,
	pub untracked:  u32,
	pub conflicted: u32,
}

impl GitStatus {
	pub fn is_clean(&self) -> bool {
		self.staged + self.unstaged + self.untracked + self.conflicted == 0
	}

	/// `main`, or `@1a2b3c4` for a detached HEAD.
	pub fn head_label(&self) -> String {
		match (&self.branch, &self.oid) {
			(Some(branch), _) => branch.clone(),
			(None, Some(oid)) => format!("@{oid}"),
			(None, None) => "(无提交)".into(),
		}
	}

	/// `↑1 ↓2`, empty when in sync or without an upstream.
	pub fn sync_label(&self) -> String {
		let mut parts = Vec::new();
		if self.ahead > 0 {
			parts.push(format!("↑{}", self.ahead));
		}
		if self.behind > 0 {
			parts.push(format!("↓{}", self.behind));
		}
		parts.join(" ")
	}

	/// `+2 ~3 ?1 !1` (staged, modified, untracked, conflicted), or `✓`.
	pub fn changes_label(&self) -> String {
		if self.is_clean() {
			return "✓".into();
		}
		let mut parts = Vec::new();
		for (n, sign) in [(self.staged, "+"), (self.unstaged, "~"), (self.untracked, "?"), (self.conflicted, "!")] {
			if n > 0 {
				parts.push(format!("{sign}{n}"));
			}
		}
		parts.join(" ")
	}

	pub fn tooltip(&self) -> String {
		let mut lines = vec![match &self.branch {
			Some(branch) => format!("分支 {branch}"),
			None => format!("分离 HEAD {}", self.oid.as_deref().unwrap_or("—")),
		}];
		match &self.upstream {
			Some(up) => lines.push(format!("上游 {up}（领先 {}，落后 {}）", self.ahead, self.behind)),
			None => lines.push("无上游分支".into()),
		}
		if self.is_clean() {
			lines.push("工作区干净".into());
		} else {
			lines.push(format!(
				"已暂存 {} · 未暂存 {} · 未跟踪 {} · 冲突 {}",
				self.staged, self.unstaged, self.untracked, self.conflicted
			));
		}
		lines.join("\n")
	}
}

/// Parses `git status --porcelain=v2 --branch` output.
pub fn parse(output: &str) -> GitStatus {
	let mut s = GitStatus::default();
	for line in output.lines() {
		if let Some(header) = line.strip_prefix("# ") {
			let (key, value) = header.split_once(' ').unwrap_or((header, ""));
			match key {
				"branch.oid" if value != "(initial)" => s.oid = Some(value.chars().take(7).collect()),
				"branch.head" if value != "(detached)" => s.branch = Some(value.to_owned()),
				"branch.upstream" => s.upstream = Some(value.to_owned()),
				"branch.ab" => {
					for part in value.split_whitespace() {
						if let Some(n) = part.strip_prefix('+') {
							s.ahead = n.parse().unwrap_or(0);
						} else if let Some(n) = part.strip_prefix('-') {
							s.behind = n.parse().unwrap_or(0);
						}
					}
				},
				_ => {},
			}
			continue;
		}
		let mut fields = line.splitn(3, ' ');
		match (fields.next(), fields.next()) {
			(Some("1" | "2"), Some(xy)) => {
				let mut xy = xy.chars();
				if xy.next().is_some_and(|c| c != '.') {
					s.staged += 1;
				}
				if xy.next().is_some_and(|c| c != '.') {
					s.unstaged += 1;
				}
			},
			(Some("u"), _) => s.conflicted += 1,
			(Some("?"), _) => s.untracked += 1,
			_ => {},
		}
	}
	s
}

/// `None` outside a repository or without git.
pub fn status(cwd: &Path) -> Option<GitStatus> {
	let output = Command::new("git")
		.args(["--no-optional-locks", "status", "--porcelain=v2", "--branch"])
		.current_dir(cwd)
		.output()
		.ok()?;
	output.status.success().then(|| parse(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[::core::prelude::v1::test]
	fn parses_branch_and_changes() {
		let s = parse(
			"# branch.oid 1234567890abcdef\n# branch.head feat/x\n# branch.upstream origin/feat/x\n# branch.ab +2 -1\n\
			 1 M. N... 100644 100644 100644 a b src/a.rs\n1 .M N... 100644 100644 100644 a b src/b.rs\n\
			 1 MM N... 100644 100644 100644 a b src/c.rs\nu UU N... 1 2 3 4 a b c d src/d.rs\n? new.rs\n",
		);
		assert_eq!(s.head_label(), "feat/x");
		assert_eq!(s.sync_label(), "↑2 ↓1");
		assert_eq!(s.changes_label(), "+2 ~2 ?1 !1");
	}

	#[::core::prelude::v1::test]
	fn detached_and_clean() {
		let s = parse("# branch.oid abcdef0123\n# branch.head (detached)\n");
		assert_eq!(s.head_label(), "@abcdef0");
		assert!(s.is_clean());
		assert_eq!(s.changes_label(), "✓");
		assert_eq!(s.sync_label(), "");
		let initial = parse("# branch.oid (initial)\n# branch.head main\n");
		assert_eq!(initial.oid, None);
		assert_eq!(initial.head_label(), "main");
	}
}
