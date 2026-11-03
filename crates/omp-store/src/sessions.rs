//! Read-only listing of saved omp sessions.
//!
//! The RPC protocol has no "list sessions" command, so this reads the session
//! directory itself. Two layouts exist:
//! - default (`~/.omp/agent/sessions`): `<root>/<encoded-cwd>/<file>.jsonl`;
//! - `--session-dir <root>`: `<root>/<file>.jsonl`.
//!
//! A session's artifacts (subagent transcripts, tool logs) live in a directory
//! named like the session file without `.jsonl`; such directories are skipped.
//!
//! Only the first [`HEADER_BYTES`] of each file are read: an optional
//! `{"type":"title"}` line, then `{"type":"session","version","id","cwd",…}`.
//! This is omp's internal format, not a public contract, so parsing is lenient:
//! files that do not parse, or carry an unsupported `version`, are skipped and
//! only counted.

use std::{
	fs,
	io::Read,
	path::{Path, PathBuf},
	time::SystemTime,
};

use serde_json::Value;

pub const HEADER_BYTES: usize = 4096;

/// Session header versions this parser understands.
pub const SUPPORTED_VERSIONS: &[u64] = &[3];

#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
	pub path:     PathBuf,
	pub id:       String,
	pub cwd:      PathBuf,
	/// Trimmed, non-empty title (`title` line first, then the header's).
	pub title:    Option<String>,
	/// Header `timestamp` (ISO 8601), when present.
	pub created:  Option<String>,
	/// Id of the session this one was forked from.
	pub parent:   Option<String>,
	pub modified: SystemTime,
}

impl SessionInfo {
	/// Last four characters of the id, for display (`#8f3a`).
	pub fn short_id(&self) -> &str {
		let start = self.id.char_indices().rev().nth(3).map_or(0, |(i, _)| i);
		&self.id[start..]
	}

	fn matches(&self, needle: &str) -> bool {
		needle.is_empty()
			|| self.title.as_deref().is_some_and(|t| t.to_lowercase().contains(needle))
			|| self.cwd.to_string_lossy().to_lowercase().contains(needle)
			|| self.id.to_lowercase().contains(needle)
	}
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
	/// Newest first.
	pub sessions: Vec<SessionInfo>,
	/// Non-empty files that could not be parsed or have an unsupported version.
	pub skipped:  usize,
}

/// `$PI_CODING_AGENT_DIR/sessions`, else `~/.omp/agent/sessions`.
pub fn default_root() -> Option<PathBuf> {
	if let Some(dir) = std::env::var_os("PI_CODING_AGENT_DIR").filter(|d| !d.is_empty()) {
		return Some(PathBuf::from(dir).join("sessions"));
	}
	let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
	Some(PathBuf::from(home).join(".omp/agent/sessions"))
}

/// Lists every session under `root`. A missing root is an empty list.
pub fn scan(root: &Path) -> Scan {
	// Canonical paths, so they compare equal to an open tab's session file.
	let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
	let mut scan = Scan::default();
	let Ok(entries) = fs::read_dir(&root) else {
		return scan;
	};
	for entry in entries.flatten() {
		let path = entry.path();
		let Ok(kind) = entry.file_type() else { continue };
		if kind.is_dir() {
			let mut file = path.clone().into_os_string();
			file.push(".jsonl");
			if Path::new(&file).is_file() {
				continue; // artifacts of a session in a flat `--session-dir`
			}
			let Ok(files) = fs::read_dir(&path) else { continue };
			for file in files.flatten() {
				read_candidate(&file.path(), &mut scan);
			}
		} else {
			read_candidate(&path, &mut scan);
		}
	}
	scan.sessions.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
	scan
}

fn read_candidate(path: &Path, scan: &mut Scan) {
	let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return };
	if name.starts_with('.') || !name.ends_with(".jsonl") || !path.is_file() {
		return;
	}
	let Ok(meta) = fs::metadata(path) else { return };
	if meta.len() == 0 {
		return;
	}
	let mut head = Vec::with_capacity(HEADER_BYTES);
	if fs::File::open(path).and_then(|f| f.take(HEADER_BYTES as u64).read_to_end(&mut head)).is_err() {
		return;
	}
	match parse_header(&String::from_utf8_lossy(&head)) {
		Some(header) => scan.sessions.push(SessionInfo {
			path:     path.to_path_buf(),
			id:       header.id,
			cwd:      header.cwd,
			title:    header.title,
			created:  header.created,
			parent:   header.parent,
			modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
		}),
		None => scan.skipped += 1,
	}
}

/// Short age for a list row: `刚刚`, `5m`, `3h`, `昨天`, `4天`.
pub fn relative_age(modified: SystemTime, now: SystemTime) -> String {
	let secs = now.duration_since(modified).map_or(0, |age| age.as_secs());
	match secs {
		0..60 => "刚刚".into(),
		60..3_600 => format!("{}m", secs / 60),
		3_600..86_400 => format!("{}h", secs / 3_600),
		86_400..172_800 => "昨天".into(),
		_ => format!("{}天", secs / 86_400),
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct Header {
	pub id:      String,
	pub cwd:     PathBuf,
	pub title:   Option<String>,
	pub created: Option<String>,
	pub parent:  Option<String>,
}

/// Parses the start of a session file: an optional `title` line, then the
/// `session` line, which must come within the first two lines.
pub fn parse_header(head: &str) -> Option<Header> {
	let mut title = None;
	// The last line may be cut at the byte limit; only complete lines count.
	let complete = head.rfind('\n').map_or("", |end| &head[..end]);
	for line in complete.lines().filter(|l| !l.trim().is_empty()).take(2) {
		let value: Value = serde_json::from_str(line).ok()?;
		match value.get("type").and_then(Value::as_str)? {
			"title" => title = non_empty(value.get("title")),
			"session" => {
				let version = value.get("version").and_then(Value::as_u64)?;
				if !SUPPORTED_VERSIONS.contains(&version) {
					return None;
				}
				let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
				return Some(Header {
					id:      text("id").filter(|id| !id.is_empty())?,
					cwd:     PathBuf::from(text("cwd")?),
					title:   title.or_else(|| non_empty(value.get("title"))),
					created: text("timestamp"),
					parent:  text("parentSession"),
				});
			},
			_ => return None,
		}
	}
	None
}

fn non_empty(value: Option<&Value>) -> Option<String> {
	value.and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
}

/// Sessions of one working directory, newest first.
#[derive(Debug, PartialEq)]
pub struct Group<'a> {
	pub cwd:      &'a Path,
	pub sessions: Vec<&'a SessionInfo>,
}

/// Groups `sessions` (newest first) by cwd, keeping only those matching
/// `query` (case-insensitive substring of title, cwd, or id). Groups are
/// ordered by their newest session.
pub fn group<'a>(sessions: &'a [SessionInfo], query: &str) -> Vec<Group<'a>> {
	let needle = query.trim().to_lowercase();
	let mut groups: Vec<Group<'a>> = Vec::new();
	for session in sessions.iter().filter(|s| s.matches(&needle)) {
		match groups.iter_mut().find(|g| g.cwd == session.cwd) {
			Some(group) => group.sessions.push(session),
			None => groups.push(Group { cwd: &session.cwd, sessions: vec![session] }),
		}
	}
	groups
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use super::*;

	fn header(id: &str, cwd: &str, extra: &str) -> String {
		format!(
			"{{\"type\":\"title\",\"v\":1,\"title\":\" {id} title \",\"pad\":\"   \"}}\n\
			 {{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"timestamp\":\"2026-11-01T00:00:00.000Z\",\"cwd\":\"{cwd}\"{extra}}}\n\
			 {{\"type\":\"model_change\"}}\n"
		)
	}

	#[test]
	fn parses_title_and_session_lines() {
		let h = parse_header(&header("abc", "/w", ",\"parentSession\":\"p1\"")).unwrap();
		assert_eq!(h.id, "abc");
		assert_eq!(h.cwd, PathBuf::from("/w"));
		assert_eq!(h.title.as_deref(), Some("abc title"));
		assert_eq!(h.parent.as_deref(), Some("p1"));
	}

	#[test]
	fn empty_title_line_falls_back_to_header_title() {
		let text = "{\"type\":\"title\",\"title\":\"\"}\n{\"type\":\"session\",\"version\":3,\"id\":\"x\",\"cwd\":\"/w\",\"title\":\"From header\"}\n";
		assert_eq!(parse_header(text).unwrap().title.as_deref(), Some("From header"));
		let untitled = "{\"type\":\"session\",\"version\":3,\"id\":\"x\",\"cwd\":\"/w\"}\n";
		assert_eq!(parse_header(untitled).unwrap().title, None);
	}

	#[test]
	fn rejects_unknown_versions_and_garbage() {
		assert_eq!(parse_header("{\"type\":\"session\",\"version\":2,\"id\":\"x\",\"cwd\":\"/w\"}\n"), None);
		assert_eq!(parse_header("not json\n"), None);
		assert_eq!(parse_header("{\"type\":\"message\"}\n"), None);
		// Cut before the session line ends.
		assert_eq!(parse_header("{\"type\":\"session\",\"version\":3,\"id\":\"x\",\"cw"), None);
	}

	#[test]
	fn scans_both_layouts_and_skips_artifacts() {
		let root = std::env::temp_dir().join(format!("omp-store-scan-{}", std::process::id()));
		_ = fs::remove_dir_all(&root);
		let project = root.join("-w-a");
		fs::create_dir_all(project.join("2026_s1")).unwrap();
		fs::write(project.join("2026_s1.jsonl"), header("s1", "/w/a", "")).unwrap();
		// Subagent transcript inside the session's artifact dir: not listed.
		fs::write(project.join("2026_s1").join("Scout.jsonl"), header("sub", "/w/a", "")).unwrap();
		fs::write(project.join(".2026_s1.jsonl.lock.os"), "").unwrap();
		std::thread::sleep(Duration::from_millis(20));
		// Flat layout: a session file directly under root, with its own artifact dir.
		fs::write(root.join("2026_s2.jsonl"), header("s2", "/w/b", "")).unwrap();
		fs::create_dir_all(root.join("2026_s2")).unwrap();
		fs::write(root.join("2026_s2").join("__advisor.jsonl"), header("adv", "/w/b", "")).unwrap();
		fs::write(project.join("broken.jsonl"), "garbage\n").unwrap();
		fs::write(project.join("empty.jsonl"), "").unwrap();

		let scan = scan(&root);
		let ids: Vec<_> = scan.sessions.iter().map(|s| s.id.as_str()).collect();
		assert_eq!(ids, ["s2", "s1"], "newest first, artifacts excluded");
		assert_eq!(scan.skipped, 1, "only the non-empty unparseable file counts");
		_ = fs::remove_dir_all(&root);
	}

	#[test]
	fn formats_relative_age() {
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
		let ago = |secs| relative_age(now - Duration::from_secs(secs), now);
		assert_eq!(ago(5), "刚刚");
		assert_eq!(ago(300), "5m");
		assert_eq!(ago(7_200), "2h");
		assert_eq!(ago(90_000), "昨天");
		assert_eq!(ago(400_000), "4天");
		// A file modified "in the future" (clock skew) is new.
		assert_eq!(relative_age(now + Duration::from_secs(9), now), "刚刚");
	}

	#[test]
	fn groups_by_cwd_and_filters() {
		let at = |secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
		let info = |id: &str, cwd: &str, title: Option<&str>, secs| SessionInfo {
			path:     PathBuf::from(format!("/r/{id}.jsonl")),
			id:       id.into(),
			cwd:      cwd.into(),
			title:    title.map(Into::into),
			created:  None,
			parent:   None,
			modified: at(secs),
		};
		let sessions = vec![
			info("c", "/w/b", Some("Fix login"), 30),
			info("b", "/w/a", Some("Plan client"), 20),
			info("a", "/w/b", None, 10),
		];
		let all = group(&sessions, "");
		assert_eq!(all.len(), 2);
		assert_eq!(all[0].cwd, Path::new("/w/b"));
		assert_eq!(all[0].sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["c", "a"]);
		let hits = group(&sessions, " LOGIN ");
		assert_eq!(hits.len(), 1);
		assert_eq!(hits[0].sessions[0].id, "c");
		assert_eq!(group(&sessions, "/w/a")[0].sessions[0].id, "b");
		assert_eq!(sessions[0].short_id(), "c");
		assert_eq!(info("01a186cf-8f3a", "/", None, 0).short_id(), "8f3a");
	}
}
