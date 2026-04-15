use std::{
	sync::mpsc,
	thread,
	time::{Duration, Instant},
};

pub enum TimeoutError {
	Panic,
	Timeout,
}

pub fn timeout<T: std::marker::Send + 'static, F: FnOnce() -> T + std::marker::Send + 'static>(worker: F, dur: Duration) -> Result<T, TimeoutError> {
	let (tx, rx) = mpsc::channel();
	thread::spawn(move || tx.send(worker()).unwrap());
	let start = Instant::now();
	loop {
		match rx.try_recv() {
			Ok(out) => return Ok(out),
			Err(mpsc::TryRecvError::Empty) => {
				if start.elapsed() > dur {
					return Err(TimeoutError::Timeout);
				}
			}
			Err(mpsc::TryRecvError::Disconnected) => {
				return Err(TimeoutError::Panic);
			}
		}
	}
}