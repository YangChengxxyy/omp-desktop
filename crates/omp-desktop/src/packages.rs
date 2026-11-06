//! omp packages (extensions, skills, themes installed with `omp install`):
//! listed, installed, updated, and removed through omp's own CLI, which also
//! keeps `settings.json` in sync. Calls block; run them off the UI thread.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use omp_bridge::SessionOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
	User,
	Project,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
	/// As written in settings: `npm:@foo/bar`, `git:github.com/u/r`, a path.
	pub source:    String,
	pub scope:     Scope,
	/// Where it is installed, when it is.
	pub path:      Option<PathBuf>,
	/// Resources narrowed with `omp config`.
	pub filtered:  bool,
}

impl Package {
	/// `npm`, `git`, or `本地`.
	pub fn kind(&self) -> &'static str {
		if self.source.starts_with("npm:") {
			"npm"
		} else if self.source.starts_with("git:") || self.source.contains("://") {
			"git"
		} else {
			"本地"
		}
	}

	/// The source without its `npm:` / `git:` prefix.
	pub fn name(&self) -> &str {
		self.source.strip_prefix("npm:").or_else(|| self.source.strip_prefix("git:")).unwrap_or(&self.source)
	}

	/// The installed version, from the package's `package.json`.
	pub fn version(&self) -> Option<String> {
		let text = std::fs::read_to_string(self.path.as_ref()?.join("package.json")).ok()?;
		let json: serde_json::Value = serde_json::from_str(&text).ok()?;
		json.get("version")?.as_str().map(str::to_owned)
	}

	pub fn description(&self) -> Option<String> {
		let text = std::fs::read_to_string(self.path.as_ref()?.join("package.json")).ok()?;
		let json: serde_json::Value = serde_json::from_str(&text).ok()?;
		json.get("description")?.as_str().map(str::to_owned).filter(|d| !d.trim().is_empty())
	}
}

/// Parses `omp list`: section headers, then `  <source>[ (filtered)]` lines,
/// each optionally followed by an indented install path.
pub fn parse_list(text: &str) -> Vec<Package> {
	let mut packages: Vec<Package> = Vec::new();
	let mut scope = None;
	for raw in text.lines() {
		let line = strip_ansi(raw);
		if line.trim().is_empty() {
			continue;
		}
		match line.trim() {
			"User packages:" => {
				scope = Some(Scope::User);
				continue;
			},
			"Project packages:" => {
				scope = Some(Scope::Project);
				continue;
			},
			_ => {},
		}
		let Some(scope) = scope else { continue };
		let indent = line.len() - line.trim_start().len();
		let body = line.trim();
		if indent >= 4 {
			if let Some(last) = packages.last_mut()
				&& last.path.is_none()
			{
				last.path = Some(PathBuf::from(body));
			}
			continue;
		}
		let (source, filtered) = match body.strip_suffix(" (filtered)") {
			Some(source) => (source, true),
			None => (body, false),
		};
		packages.push(Package { source: source.to_owned(), scope, path: None, filtered });
	}
	packages
}

fn strip_ansi(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut chars = text.chars();
	while let Some(c) = chars.next() {
		if c == '\u{1b}' {
			if chars.next() == Some('[') {
				for c in chars.by_ref() {
					if c.is_ascii_alphabetic() {
						break;
					}
				}
			}
			continue;
		}
		out.push(c);
	}
	out
}

/// Runs `omp <args>` in `cwd` (project packages are read from there) and
/// returns stdout, or the error output when it fails.
fn run_omp(options: &SessionOptions, cwd: &Path, args: &[&str]) -> Result<String, String> {
	let mut command = Command::new(&options.omp);
	command
		.args(args)
		// Never prompt for project trust; project packages are listed only if trusted.
		.arg("--no-approve")
		.current_dir(cwd)
		.env("NO_COLOR", "1")
		.stdin(Stdio::null());
	for (key, value) in &options.env {
		command.env(key, value);
	}
	let output = command.output().map_err(|error| format!("无法运行 {}：{error}", options.omp.display()))?;
	let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
	if output.status.success() {
		return Ok(stdout);
	}
	let stderr = strip_ansi(String::from_utf8_lossy(&output.stderr).trim());
	Err(if stderr.is_empty() { strip_ansi(stdout.trim()) } else { stderr })
}

pub fn list(options: &SessionOptions, cwd: &Path) -> Result<Vec<Package>, String> {
	run_omp(options, cwd, &["list"]).map(|text| parse_list(&text))
}

pub fn install(options: &SessionOptions, cwd: &Path, source: &str) -> Result<(), String> {
	run_omp(options, cwd, &["install", source]).map(drop)
}

pub fn remove(options: &SessionOptions, cwd: &Path, package: &Package) -> Result<(), String> {
	let mut args = vec!["remove", package.source.as_str()];
	if package.scope == Scope::Project {
		args.push("-l");
	}
	run_omp(options, cwd, &args).map(drop)
}

/// Updates one package, or all of them when `source` is `None`.
pub fn update(options: &SessionOptions, cwd: &Path, source: Option<&str>) -> Result<(), String> {
	match source {
		Some(source) => run_omp(options, cwd, &["update", source]),
		None => run_omp(options, cwd, &["update", "--extensions"]),
	}
	.map(drop)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[::core::prelude::v1::test]
	fn parses_omp_list() {
		let text = "\u{1b}[1mUser packages:\u{1b}[22m\n  npm:@foo/bar\n\u{1b}[2m    /home/u/.omp/agent/npm/node_modules/@foo/bar\u{1b}[22m\n  git:github.com/u/r (filtered)\n\nProject packages:\n  ./local/ext\n    /p/local/ext\n";
		let packages = parse_list(text);
		assert_eq!(packages.len(), 3);
		assert_eq!(packages[0].source, "npm:@foo/bar");
		assert_eq!(packages[0].name(), "@foo/bar");
		assert_eq!(packages[0].kind(), "npm");
		assert_eq!(packages[0].path.as_deref(), Some(Path::new("/home/u/.omp/agent/npm/node_modules/@foo/bar")));
		assert!(packages[1].filtered);
		assert_eq!(packages[1].kind(), "git");
		assert_eq!(packages[1].path, None);
		assert_eq!(packages[2].scope, Scope::Project);
		assert_eq!(packages[2].kind(), "本地");
		assert!(parse_list("No packages installed.\n").is_empty());
	}
}
