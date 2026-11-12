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
  /// The main frame's address changed, including same-document navigation.
  Address(String),
  /// The page entered or left HTML fullscreen. A browser with a window of its
  /// own is taken fullscreen by the runtime; an embedded surface guest has
  /// none, so its embedder decides what filling the screen means.
  Fullscreen(bool),
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
  /// Progress of one download this browser started. Reported until it ends.
  Download(DownloadProgress),
  /// The page opened a JavaScript dialog and is blocked until
  /// [`answer_dialog`](crate::answer_dialog).
  Dialog {
    id: u64,
    kind: DialogKind,
    origin: String,
    message: String,
    /// The initial value of a prompt.
    default_text: String,
  },
  /// A pending dialog ended without an answer, e.g. the page went away.
  DialogDismissed { id: u64 },
  /// The user asked for the context menu of something in the page. Nothing is
  /// shown until the embedder shows it; the request waits for
  /// [`answer_context_menu`](crate::answer_context_menu).
  ContextMenu(ContextMenu),
  /// A server or proxy asks for a user name and password; the request waits
  /// for [`answer_auth`](crate::answer_auth). Reported from CEF's IO thread.
  AuthRequest {
    id: u64,
    host: String,
    port: u16,
    realm: String,
    is_proxy: bool,
  },
}

/// What a context menu was asked for, and the entries the browser offers there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMenu {
  pub id: u64,
  /// Where, in logical points, as CEF reports it: from the top left corner of
  /// the view the page is shown in, which for an embedded surface guest is the
  /// window it is composited into.
  pub x: i32,
  pub y: i32,
  /// The link under the pointer; empty when there is none.
  pub link_url: String,
  /// The image under the pointer; empty when there is none.
  pub image_url: String,
  pub selection_text: String,
  pub editable: bool,
  pub items: Vec<ContextMenuItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMenuItem {
  /// What [`answer_context_menu`](crate::answer_context_menu) is given to run
  /// this entry. A `MENU_ID_*` value for the browser's own entries.
  pub command: i32,
  /// As the browser words it; an entry an extension added carries its title.
  pub label: String,
  pub kind: ContextMenuItemKind,
  pub enabled: bool,
  pub checked: bool,
  /// The entries of a submenu.
  pub items: Vec<ContextMenuItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuItemKind {
  Command,
  Check,
  Radio,
  Separator,
  Submenu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
  Alert,
  Confirm,
  Prompt,
  /// The page asks whether to leave it; it supplies no message of its own.
  BeforeUnload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadProgress {
  /// Identifies the download for [`cancel_download`](crate::cancel_download).
  pub id: u32,
  pub url: String,
  /// Where the file is being written; empty until a destination is chosen.
  pub path: String,
  pub received_bytes: i64,
  /// Zero or negative when the server did not announce a size.
  pub total_bytes: i64,
  pub state: DownloadState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadState {
  InProgress,
  Complete,
  Canceled,
  /// Stopped by a network or file error.
  Interrupted,
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
