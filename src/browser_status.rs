// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Native browser chrome state: what a tab strip and toolbar show for one
//! browser. Every value is reported by CEF for the exact native browser the
//! observer was registered for; nothing here is derived or estimated.

/// A key pressed together with a platform shortcut modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
  /// Layout-independent Windows virtual key code, as CEF reports it.
  pub key_code: i32,
  pub command: bool,
  pub control: bool,
  pub shift: bool,
  pub alt: bool,
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum BrowserStatus {
  /// The browser started or stopped loading, with its session history bounds.
  Loading {
    is_loading: bool,
    can_go_back: bool,
    can_go_forward: bool,
  },
  /// Overall load progress in `0.0..=1.0`.
  Progress(f64),
  /// Candidate icon URLs of the current document, in page order.
  Favicon(Vec<String>),
  /// The main frame failed to load. An aborted navigation is not a failure.
  LoadFailed {
    url: String,
    code: i32,
    description: String,
  },
  /// A chord the shortcut filter claimed. The page did not act on the key.
  Shortcut(KeyChord),
  /// Progress of the find-in-page search started on this browser's host.
  /// Chromium reports the two numbers independently: `None` means that
  /// number is unchanged by this update.
  Find {
    count: Option<u32>,
    /// 1-based ordinal of the highlighted match; 0 when there is none.
    active: Option<u32>,
    final_update: bool,
  },
  /// The page asks for permissions and waits for
  /// [`answer_permission_request`](crate::answer_permission_request).
  PermissionRequest {
    id: u64,
    origin: String,
    kinds: Vec<tauri_runtime::webview::PermissionKind>,
  },
  /// A pending request ended without an answer, e.g. the page navigated away.
  PermissionDismissed { id: u64 },
}

/// Whether, and when, the embedder takes a chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutClaim {
  /// The chord is the page's.
  Pass,
  /// Taken before the page sees the key.
  Claim,
  /// Taken only when the page did not handle the key, so a page that implements
  /// the same shortcut itself keeps it.
  ClaimIfUnhandled,
}

pub type BrowserStatusHandler = dyn Fn(BrowserStatus) + Send + Sync;

/// Decides, on the UI thread, whether the embedder claims a chord before the
/// page sees it. Only raw key-down events carrying the command or control
/// modifier are offered, so text input and IME composition never pass through.
pub type ShortcutFilter = dyn Fn(KeyChord) -> ShortcutClaim + Send + Sync;

/// A status tagged with the native browser that produced it, so one client
/// never reports a DevTools browser routed through it.
pub(crate) type ScopedStatus = dyn Fn(i32, BrowserStatus) + Send + Sync;

/// `net::ERR_ABORTED`: the navigation was replaced or stopped.
pub(crate) const ERR_ABORTED: i32 = -3;
