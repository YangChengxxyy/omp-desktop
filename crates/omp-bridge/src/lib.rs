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

use omp_rpc::{Client, ClientOptions, Command, Event, SetAskDialogCommand};
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
	Rpc(omp_rpc::Error),
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
}

impl Default for SessionOptions {
	fn default() -> Self {
		Self { omp: PathBuf::from("omp"), cwd: None, resume: None, session_dir: None, extra_args: Vec::new() }
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
		let (client, receiver) = Client::spawn(process, ClientOptions::default())?;
		let supported = client.ready().supported_protocol_versions.clone().unwrap_or_default();
		if client.protocol_version() != 2 {
			return Err(BridgeError::ProtocolUnsupported { supported });
		}

		let filter = client.call(&SetEventFilterDelta { events: None, message_updates: "delta" })?;
		let delta = filter.get("messageUpdates").and_then(Value::as_str) == Some("delta");
		client.call(&SetAskDialogCommand { enabled: true })?;

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
}
