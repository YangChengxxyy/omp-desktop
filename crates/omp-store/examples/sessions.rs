//! Prints the saved sessions omp-desktop would list, grouped by cwd.
//!
//! cargo run -p omp-store --example sessions [-- <root> [query]]

fn main() {
	let mut args = std::env::args().skip(1);
	let root = args.next().map(Into::into).or_else(omp_store::sessions::default_root).expect("no session root");
	let query = args.next().unwrap_or_default();
	let scan = omp_store::sessions::scan(&root);
	for group in omp_store::sessions::group(&scan.sessions, &query) {
		println!("{} ({})", group.cwd.display(), group.sessions.len());
		for s in group.sessions {
			println!("  #{} {}", s.short_id(), s.title.as_deref().unwrap_or("(未命名)"));
		}
	}
	println!("skipped: {}", scan.skipped);
}
