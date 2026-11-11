// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! HTTP authentication answered by the embedder.
//!
//! Chrome's login prompt is a dialog parented to the browser's window. A
//! browser without one has nowhere to show it, so a `401` there aborts a debug
//! build. A webview that opts in receives the challenge as a browser status and
//! answers it with [`answer_auth`].

use cef::*;

struct PendingAuth {
  browser_id: i32,
  callback: AuthCallback,
}

/// CEF asks on its IO thread and the embedder answers from any thread; the
/// callback is a thread-safe reference-counted object. It is never released
/// while the registry is locked.
struct PendingAuths(std::collections::HashMap<u64, PendingAuth>);
// SAFETY: see above; the mutex guards the map and CEF guards the callback.
unsafe impl Send for PendingAuths {}
static PENDING: std::sync::Mutex<Option<PendingAuths>> = std::sync::Mutex::new(None);
static NEXT_AUTH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn with_pending<T>(
  action: impl FnOnce(&mut std::collections::HashMap<u64, PendingAuth>) -> T,
) -> T {
  let mut pending = PENDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  action(&mut pending.get_or_insert_with(|| PendingAuths(Default::default())).0)
}

/// Offer a challenge to the embedder. Returns the id it will be answered by.
pub(crate) fn remember(browser_id: i32, callback: &AuthCallback) -> u64 {
  let id = NEXT_AUTH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
  let replaced = with_pending(|pending| {
    pending.insert(
      id,
      PendingAuth {
        browser_id,
        callback: callback.clone(),
      },
    )
  });
  drop(replaced);
  id
}

/// Answer a [`BrowserStatus::AuthRequest`](crate::BrowserStatus) with a user
/// name and password, or cancel it with `None`, which lets the server's own
/// "unauthorized" page load. Returns `false` when it is no longer pending.
pub fn answer_auth(id: u64, credentials: Option<(&str, &str)>) -> bool {
  let Some(auth) = with_pending(|pending| pending.remove(&id)) else {
    return false;
  };
  match credentials {
    Some((username, password)) => auth.callback.cont(
      Some(&CefString::from(username)),
      Some(&CefString::from(password)),
    ),
    None => auth.callback.cancel(),
  }
  true
}

/// A browser that closes cancels its unanswered challenges.
pub(crate) fn forget_browser(browser_id: i32) {
  let dropped = with_pending(|pending| {
    let ids: Vec<u64> = pending
      .iter()
      .filter(|(_, auth)| auth.browser_id == browser_id)
      .map(|(id, _)| *id)
      .collect();
    ids
      .into_iter()
      .filter_map(|id| pending.remove(&id))
      .collect::<Vec<_>>()
  });
  for auth in dropped {
    auth.callback.cancel();
  }
}
