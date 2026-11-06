//! Launch environment. An app opened from Finder or the Dock inherits
//! launchd's environment: `PATH=/usr/bin:/bin:/usr/sbin:/sbin` and cwd `/`.
//! omp is usually in `/opt/homebrew/bin`, and its tools (git, node, …) need
//! the user's shell PATH too, so before the UI starts we take PATH from a
//! login shell, the way terminal-launched sessions see it.

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const MARKER: &str = "__OMP_DESKTOP_PATH__";
/// A slow `.zshrc` must not hold up the window for long.
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);
/// Where omp installers put the binary, tried after PATH.
const FALLBACK_DIRS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "~/.local/bin"];

/// True when running from an `.app` bundle. Finder, the Dock, and `open`
/// launch it that way, with an environment that may or may not be the
/// terminal's (`open` passes some of it), so the bundle is the signal.
pub fn in_app_bundle() -> bool {
	std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"))
}

/// Replaces PATH with the login shell's when running from the app bundle.
/// Call before any other thread starts (it uses `set_var`).
pub fn import_shell_path() {
	if !in_app_bundle() {
		return;
	}
	let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
	match shell_path(Path::new(&shell)) {
		// SAFETY: called first thing in `main`, before GPUI or any other thread.
		Some(path) => unsafe { std::env::set_var("PATH", merge_path(&path, std::env::var_os("PATH"))) },
		None => eprintln!("omp-desktop: could not read PATH from {}", shell.to_string_lossy()),
	}
}

/// The working directory for the first tab when none was given: cwd, unless
/// launched from the GUI (cwd `/`), then the directory saved in
/// `~/Library/Application Support/omp-desktop/default-cwd` if it exists, else home.
pub fn default_cwd() -> PathBuf {
	let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
	if cwd != Path::new("/") {
		return cwd;
	}
	if let Some(dir) = configured_cwd() {
		return dir;
	}
	home().unwrap_or(cwd)
}

/// The saved launch directory (one path, `~/` allowed), when it is a directory.
pub fn configured_cwd() -> Option<PathBuf> {
	let file = default_cwd_file()?;
	let text = std::fs::read_to_string(file).ok()?;
	let dir = expand_home(text.trim());
	(!text.trim().is_empty() && dir.is_dir()).then_some(dir)
}

fn default_cwd_file() -> Option<PathBuf> {
	Some(home()?.join("Library/Application Support/omp-desktop/default-cwd"))
}

/// Saves the launch directory for GUI launches; `None` forgets it (home is
/// used again).
pub fn save_default_cwd(dir: Option<&Path>) -> std::io::Result<()> {
	if cfg!(test) {
		return Ok(());
	}
	let file = default_cwd_file().ok_or_else(|| std::io::Error::other("HOME 未设置"))?;
	match dir {
		Some(dir) => {
			if let Some(parent) = file.parent() {
				std::fs::create_dir_all(parent)?;
			}
			std::fs::write(&file, dir.to_string_lossy().as_bytes())
		},
		None => match std::fs::remove_file(&file) {
			Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
			_ => Ok(()),
		},
	}
}

pub fn home_dir() -> Option<PathBuf> {
	home()
}
/// The omp binary: first match on PATH, then the usual install directories,
/// else plain `omp` so the "not installed" screen names it.
pub fn locate_omp() -> PathBuf {
	let path = std::env::var_os("PATH").unwrap_or_default();
	let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
	dirs.extend(FALLBACK_DIRS.iter().map(|dir| expand_home(dir)));
	find_in(&dirs, "omp").unwrap_or_else(|| PathBuf::from("omp"))
}

/// Runs `$SHELL -l -i -c` and returns the PATH it prints. Interactive so
/// `.zshrc` additions count; the marker skips anything the rc files print.
fn shell_path(shell: &Path) -> Option<String> {
	let mut child = Command::new(shell)
		.args(["-l", "-i", "-c", &format!("printf '{MARKER}%s{MARKER}' \"$PATH\"")])
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.ok()?;
	let mut stdout = child.stdout.take()?;
	let (tx, rx) = std::sync::mpsc::channel();
	// Read on a thread so a chatty rc file cannot fill the pipe and stall.
	std::thread::spawn(move || {
		let mut out = String::new();
		_ = stdout.read_to_string(&mut out);
		_ = tx.send(out);
	});
	let out = rx.recv_timeout(SHELL_TIMEOUT);
	_ = child.kill();
	_ = child.wait();
	parse_marked(&out.ok()?)
}

fn parse_marked(out: &str) -> Option<String> {
	let start = out.find(MARKER)? + MARKER.len();
	let len = out[start..].find(MARKER)?;
	let path = out[start..start + len].trim();
	(!path.is_empty()).then(|| path.to_owned())
}

/// The shell's entries first, then any inherited ones it lacks.
fn merge_path(shell: &str, inherited: Option<OsString>) -> OsString {
	let mut dirs: Vec<PathBuf> = std::env::split_paths(shell).collect();
	for dir in std::env::split_paths(&inherited.unwrap_or_default()) {
		if !dirs.contains(&dir) {
			dirs.push(dir);
		}
	}
	std::env::join_paths(dirs).unwrap_or_else(|_| shell.into())
}

fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
	use std::os::unix::fs::PermissionsExt as _;
	dirs.iter().map(|dir| dir.join(name)).find(|file| {
		std::fs::metadata(file).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
	})
}

fn home() -> Option<PathBuf> {
	std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn expand_home(dir: &str) -> PathBuf {
	match (dir.strip_prefix("~/"), home()) {
		(Some(rest), Some(home)) => home.join(rest),
		_ => PathBuf::from(dir),
	}
}

#[cfg(test)]
mod tests {
	use super::{find_in, merge_path, parse_marked, shell_path, MARKER};
	use std::path::{Path, PathBuf};

	#[test]
	fn parses_marked_path_and_merges() {
		let noisy = format!("Welcome!\n{MARKER}/opt/homebrew/bin:/usr/bin{MARKER}\nbye");
		assert_eq!(parse_marked(&noisy).as_deref(), Some("/opt/homebrew/bin:/usr/bin"));
		assert_eq!(parse_marked("no marker"), None);
		assert_eq!(parse_marked(&format!("{MARKER}{MARKER}")), None);
		let merged = merge_path("/opt/homebrew/bin:/usr/bin", Some("/usr/bin:/sbin".into()));
		assert_eq!(merged, "/opt/homebrew/bin:/usr/bin:/sbin");
	}

	#[test]
	fn finds_executables_only() {
		use std::os::unix::fs::PermissionsExt as _;
		let dir = std::env::temp_dir().join(format!("omp-desktop-locate-{}", std::process::id()));
		_ = std::fs::remove_dir_all(&dir);
		let (plain, bin) = (dir.join("plain"), dir.join("bin"));
		std::fs::create_dir_all(&plain).unwrap();
		std::fs::create_dir_all(&bin).unwrap();
		std::fs::write(plain.join("omp"), "").unwrap();
		std::fs::write(bin.join("omp"), "#!/bin/sh\n").unwrap();
		std::fs::set_permissions(bin.join("omp"), std::fs::Permissions::from_mode(0o755)).unwrap();
		let dirs = [PathBuf::from("/nonexistent"), plain.clone(), bin.clone()];
		assert_eq!(find_in(&dirs, "omp"), Some(bin.join("omp")), "skips the non-executable file");
		assert_eq!(find_in(&dirs[..2], "omp"), None);
		_ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn reads_path_from_a_real_shell() {
		let path = shell_path(Path::new("/bin/sh")).expect("sh prints PATH");
		assert!(path.contains("/usr/bin"), "{path}");
		assert_eq!(shell_path(Path::new("/nonexistent/shell")), None);
	}
}
