//! Runs a blocking call (an RPC round trip, a process start or teardown) on
//! its own thread and awaits the result from any executor.
//!
//! RPC calls block for as long as pi takes: a `prompt` whose extension
//! command waits on a dialog, a `bash` command, a login. On an executor
//! worker they would hold up unrelated work queued behind them, such as the
//! `abort` that should end them.

pub async fn blocking<T: Send + 'static>(call: impl FnOnce() -> T + Send + 'static) -> T {
	let (tx, rx) = async_channel::bounded(1);
	std::thread::Builder::new()
		.name("pi-call".into())
		.spawn(move || _ = tx.send_blocking(call()))
		.expect("spawn a call thread");
	// The sender drops without a value only if the call panicked.
	rx.recv().await.expect("blocking call panicked")
}
