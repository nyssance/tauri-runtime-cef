// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! A blocking getter called on the main thread outside a winit callback (an AppKit
//! notification observer, say) queues its message for the loop that thread runs. Waiting for
//! the reply must keep that loop running, or the application freezes; the wry runtime answers
//! such a call inline.

// AppKit requires the process main thread, which the standard test harness does not use.
#[cfg(target_os = "macos")]
fn main() {
  use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
  };

  use objc2::MainThreadMarker;
  use tauri_runtime_cef::__contract::{MainThreadWake, wait_for_reply};

  MainThreadMarker::new().expect("test must run on the main thread");

  thread::spawn(|| {
    thread::sleep(Duration::from_secs(20));
    eprintln!("waiting for a reply on the main thread deadlocked");
    std::process::exit(1);
  });

  // Stands in for the runtime's message queue, drained by the wake-up pass like winit's proxy.
  type Queue = Mutex<VecDeque<(u32, mpsc::Sender<u32>)>>;
  let queue: Arc<Queue> = Arc::default();
  let wake = MainThreadWake::new({
    let queue = queue.clone();
    move || {
      while let Some((value, reply)) = queue.lock().unwrap().pop_front() {
        let _ = reply.send(value * 2);
      }
    }
  });

  // Queued from the main thread itself, outside any loop pass.
  for value in 1..=3 {
    let (tx, rx) = mpsc::channel();
    queue.lock().unwrap().push_back((value, tx));
    wake.wake_up();
    assert_eq!(wait_for_reply(&rx), Some(value * 2));
  }

  // A reply from another thread, which does not wake the main loop.
  let (tx, rx) = mpsc::channel();
  thread::spawn(move || {
    thread::sleep(Duration::from_millis(200));
    tx.send(7).unwrap();
  });
  assert_eq!(wait_for_reply(&rx), Some(7));

  // A sender dropped without replying ends the wait.
  let (tx, rx) = mpsc::channel::<u32>();
  thread::spawn(move || drop(tx));
  assert_eq!(wait_for_reply(&rx), None);
}

#[cfg(not(target_os = "macos"))]
fn main() {}
