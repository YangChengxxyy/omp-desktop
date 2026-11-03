//! Process side of omp-desktop: spawns `omp --mode rpc-ui`, verifies the
//! installed omp is compatible, enables delta streaming and the `ask`
//! dialog, and moves the blocking `omp_rpc` event receiver onto an
//! [`async_channel`] the UI executor can await.

use std::{
	fmt,
	path::PathBuf,
	process::Command as Process,
	sync::Arc,
	thread,
};

use omp_rpc::{
	Client, ClientOptions, Command, Event, SetAskDialogCommand, SetSubagentSubscriptionCommand,
	SubagentSubscriptionLevel,
};
use serde::Serialize;
use serde_json::Value;

/// Oldest omp whose wire schema matches `vendor/omp-rpc` (see its UPSTREAM).
pub const MIN_OMP_VERSION: Version = Version(18, 13, 3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl fmt::Display for Version {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}.{}.{}", self.0, self.1, self.2)
	}
}

impl Version {
	/// Parses the first line of `omp --version` (`omp/18.13.3`).
	pub fn parse_omp_version(output: &str) -> Option<Self> {
		let line = output.lines().next()?.trim();
		let version = line.strip_prefix("omp/").unwrap_or(line);
		let mut parts = version.split(['.', '-', '+']).map(str::parse::<u32>);
		Some(Self(parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?))
	}
}

#[derive(Debug)]
pub enum BridgeError {
	/// `omp` could not be executed.
	NotInstalled(std::io::Error),
	/// `omp --version` printed something unparseable.
	UnknownVersion(String),
	TooOld { found: Version, required: Version },
	/// The server does not offer protocol v2 (chunked frames).
	ProtocolUnsupported { supported: Vec<i64> },
	/// The process exited during startup; `stderr` is the tail it printed.
	Exited { stderr: String },
	Rpc(omp_rpc::Error),
}

impl BridgeError {
	/// omp quit because no model has usable credentials (a fresh machine
	/// before `omp login`).
	pub fn needs_login(&self) -> bool {
		match self {
			Self::Exited { stderr } => {
				["No default model selected", "No models available", "No model available matching"]
					.iter()
					.any(|marker| stderr.contains(marker))
			},
			_ => false,
		}
	}
}

impl fmt::Display for BridgeError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::NotInstalled(error) => write!(f, "omp is not installed or not on PATH: {error}"),
			Self::UnknownVersion(output) => write!(f, "cannot parse `omp --version` output: {output:?}"),
			Self::TooOld { found, required } => {
				write!(f, "omp {found} is too old; omp-desktop needs {required} or newer")
			},
			Self::ProtocolUnsupported { supported } => {
				write!(f, "omp offers RPC protocol versions {supported:?}; omp-desktop needs 2")
			},
			Self::Exited { stderr } if stderr.trim().is_empty() => f.write_str("omp exited during startup"),
			Self::Exited { stderr } => write!(f, "omp exited during startup: {}", stderr.trim()),
			Self::Rpc(error) => write!(f, "omp rpc: {error}"),
		}
	}
}

impl std::error::Error for BridgeError {}

impl From<omp_rpc::Error> for BridgeError {
	fn from(error: omp_rpc::Error) -> Self {
		Self::Rpc(error)
	}
}

/// How to launch one session process.
#[derive(Debug, Clone)]
pub struct SessionOptions {
	pub omp:         PathBuf,
	pub cwd:         Option<PathBuf>,
	/// Session file to continue (`--resume <file>`); `None` starts a new one.
	pub resume:      Option<PathBuf>,
	/// Session storage root (`--session-dir`). `None` keeps omp's default
	/// (`~/.omp/agent/sessions/<encoded-cwd>/`); a custom root is flat.
	pub session_dir: Option<PathBuf>,
	/// Extra launch flags (`--no-session`, `--thinking off`, …).
	pub extra_args:  Vec<String>,
	/// Extra environment for the process (tests point `PI_CODING_AGENT_DIR`
	/// at a scratch directory).
	pub env:         Vec<(String, String)>,
}

impl Default for SessionOptions {
	fn default() -> Self {
		Self {
			omp:         PathBuf::from("omp"),
			cwd:         None,
			resume:      None,
			session_dir: None,
			extra_args:  Vec::new(),
			env:         Vec::new(),
		}
	}
}

impl SessionOptions {
	/// Arguments after `omp`.
	pub fn args(&self) -> Vec<std::ffi::OsString> {
		let mut args: Vec<std::ffi::OsString> = vec!["--mode".into(), "rpc-ui".into()];
		if let Some(dir) = &self.session_dir {
			args.extend(["--session-dir".into(), dir.into()]);
		}
		if let Some(file) = &self.resume {
			args.extend(["--resume".into(), file.into()]);
		}
		args.extend(self.extra_args.iter().map(Into::into));
		if let Some(cwd) = &self.cwd {
			args.extend(["--cwd".into(), cwd.into()]);
		}
		args
	}
}

/// A connected session process.
pub struct Session {
	pub client:  Arc<Client>,
	pub events:  async_channel::Receiver<Event>,
	pub version: Version,
	/// The server accepted `messageUpdates: "delta"`; otherwise updates are
	/// full snapshots. `omp_store::Transcript` handles both.
	pub delta:   bool,
}

/// Runs `omp --version` and checks it against [`MIN_OMP_VERSION`].
pub fn check_version(omp: &PathBuf) -> Result<Version, BridgeError> {
	let output = Process::new(omp).arg("--version").output().map_err(BridgeError::NotInstalled)?;
	let text = String::from_utf8_lossy(&output.stdout).into_owned();
	let version = Version::parse_omp_version(&text).ok_or(BridgeError::UnknownVersion(text))?;
	if version < MIN_OMP_VERSION {
		return Err(BridgeError::TooOld { found: version, required: MIN_OMP_VERSION });
	}
	Ok(version)
}

impl Session {
	/// Blocking: spawns the process and performs the startup handshake. Run
	/// it off the UI thread.
	pub fn spawn(options: &SessionOptions) -> Result<Self, BridgeError> {
		let version = check_version(&options.omp)?;
		let mut process = Process::new(&options.omp);
		process.args(options.args());
		process.envs(options.env.iter().map(|(k, v)| (k, v)));
		let stderr = capture_stderr(&mut process).map_err(|error| BridgeError::Rpc(omp_rpc::Error::Io(error)))?;
		let (client, receiver) = match Client::spawn(process, ClientOptions::default()) {
			Ok(connected) => connected,
			Err(omp_rpc::Error::Closed) => return Err(BridgeError::Exited { stderr: stderr.wait_tail() }),
			Err(error) => return Err(error.into()),
		};
		let supported = client.ready().supported_protocol_versions.clone().unwrap_or_default();
		if client.protocol_version() != 2 {
			return Err(BridgeError::ProtocolUnsupported { supported });
		}

		let filter = client.call(&SetEventFilterDelta { events: None, message_updates: "delta" })?;
		let delta = filter.get("messageUpdates").and_then(Value::as_str) == Some("delta");
		client.call(&SetAskDialogCommand { enabled: true })?;
		client.call(&SetSubagentSubscriptionCommand { level: SubagentSubscriptionLevel::Progress })?;

		let (tx, events) = async_channel::unbounded();
		thread::Builder::new()
			.name("omp-events".into())
			.spawn(move || {
				// Ends when the client closes (sender dropped) or the UI
				// drops the receiver.
				while let Ok(event) = receiver.recv() {
					if tx.send_blocking(event).is_err() {
						break;
					}
				}
			})
			.map_err(|error| BridgeError::Rpc(omp_rpc::Error::Io(error)))?;

		Ok(Self { client: Arc::new(client), events, version, delta })
	}
}

/// Sends SIGTERM to the process group of a session spawned by
/// [`Session::spawn`] (it leads its own group). Use it when a blocking call
/// holds the client and the process must stop now: the call then returns
/// `Closed`, and dropping the last client handle reaps the process.
pub fn terminate_group(pid: u32) {
	#[cfg(unix)]
	if let Ok(pgid) = libc::pid_t::try_from(pid) {
		// SAFETY: `kill` has no memory-safety preconditions; ESRCH (already
		// gone) is fine.
		unsafe { libc::kill(-pgid, libc::SIGTERM) };
	}
	#[cfg(not(unix))]
	let _ = pid;
}

/// The last bytes omp wrote to stderr. A thread drains the pipe (so omp
/// never blocks on it) and copies everything to this process's stderr.
struct StderrTail {
	tail:   Arc<std::sync::Mutex<Vec<u8>>>,
	reader: thread::JoinHandle<()>,
}

const STDERR_TAIL_BYTES: usize = 8 * 1024;

impl StderrTail {
	/// Waits for the pipe to close (the process exited) up to a second and
	/// returns the tail without ANSI colour codes.
	fn wait_tail(self) -> String {
		let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
		while !self.reader.is_finished() && std::time::Instant::now() < deadline {
			thread::sleep(std::time::Duration::from_millis(10));
		}
		let bytes = self.tail.lock().map(|t| t.clone()).unwrap_or_default();
		strip_ansi(&String::from_utf8_lossy(&bytes))
	}
}

fn capture_stderr(process: &mut Process) -> std::io::Result<StderrTail> {
	use std::io::{Read as _, Write as _};
	let (mut reader, writer) = std::io::pipe()?;
	process.stderr(writer);
	let tail = Arc::new(std::sync::Mutex::new(Vec::new()));
	let sink = tail.clone();
	let reader = thread::Builder::new().name("omp-stderr".into()).spawn(move || {
		let mut buffer = [0u8; 4096];
		while let Ok(read) = reader.read(&mut buffer) {
			if read == 0 {
				break;
			}
			_ = std::io::stderr().write_all(&buffer[..read]);
			if let Ok(mut tail) = sink.lock() {
				tail.extend_from_slice(&buffer[..read]);
				let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
				tail.drain(..excess);
			}
		}
	})?;
	Ok(StderrTail { tail, reader })
}

fn strip_ansi(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut chars = text.chars().peekable();
	while let Some(c) = chars.next() {
		if c == '\u{1b}' && chars.peek() == Some(&'[') {
			chars.next();
			// CSI: parameters, then one final byte in @..~.
			for c in chars.by_ref() {
				if ('@'..='~').contains(&c) {
					break;
				}
			}
		} else {
			out.push(c);
		}
	}
	out
}

/// `set_event_filter` with `messageUpdates`, which the generated
/// `SetEventFilterCommand` lacks.
#[derive(Debug, Serialize)]
struct SetEventFilterDelta {
	events:          Option<Vec<String>>,
	#[serde(rename = "messageUpdates")]
	message_updates: &'static str,
}

impl Command for SetEventFilterDelta {
	const NAME: &'static str = "set_event_filter";
	const TIMEOUT_MS: Option<u64> = None;
	type Output = Value;

	fn decode(data: Option<Value>) -> Result<Self::Output, serde_json::Error> {
		Ok(data.unwrap_or(Value::Null))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_omp_version_output() {
		assert_eq!(Version::parse_omp_version("omp/18.13.3\n"), Some(Version(18, 13, 3)));
		assert_eq!(Version::parse_omp_version("omp/19.0.0-beta.1"), Some(Version(19, 0, 0)));
		assert_eq!(Version::parse_omp_version("garbage"), None);
		assert!(Version(18, 13, 2) < MIN_OMP_VERSION);
		assert!(Version(18, 14, 0) > MIN_OMP_VERSION);
	}

	#[test]
	fn startup_exit_reports_missing_credentials() {
		assert_eq!(strip_ansi("\u{1b}[31mNo models\u{1b}[39m."), "No models.");
		let stderr = "\u{1b}[31mNo default model selected. Use /login\u{1b}[39m\n".to_owned();
		let error = BridgeError::Exited { stderr: strip_ansi(&stderr) };
		assert!(error.needs_login());
		assert!(!BridgeError::Exited { stderr: "boom".into() }.needs_login());
	}

	/// A process that exits before `ready` yields its stderr.
	#[test]
	fn exited_process_returns_stderr_tail() {
		let options = SessionOptions {
			omp: PathBuf::from("/bin/sh"),
			..Default::default()
		};
		let mut process = Process::new(&options.omp);
		process.args(["-c", "echo 'No default model selected.' >&2; exit 1"]);
		let stderr = capture_stderr(&mut process).unwrap();
		let error = match Client::spawn(process, ClientOptions::default()) {
			Err(omp_rpc::Error::Closed) => BridgeError::Exited { stderr: stderr.wait_tail() },
			other => panic!("unexpected {:?}", other.map(|_| ())),
		};
		assert!(error.needs_login(), "{error}");
	}
}
