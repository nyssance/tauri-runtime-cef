// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! JavaScript dialogs answered by the embedder.
//!
//! CEF's own dialog is a native window parented to the browser's window. A
//! browser without one — an embedded surface guest — has nowhere to show it, so
//! `alert()` there aborts a debug build and stalls the page in a release one.
//! A webview that opts in receives each dialog as a browser status instead and
//! answers it with [`answer_dialog`].

use std::sync::Arc;

use cef::*;

struct PendingDialog {
  browser_id: i32,
  callback: JsdialogCallback,
}

/// Confined to CEF's UI thread: dialogs arrive there and [`answer_dialog`]
/// requires it.
///
/// A callback is never released while the registry is locked: releasing the
/// last reference lets CEF cancel the dialog, which calls back into this module
/// on the same thread.
struct PendingDialogs(std::collections::HashMap<u64, PendingDialog>);
// SAFETY: confined to the UI thread, as documented above.
unsafe impl Send for PendingDialogs {}
static PENDING: std::sync::Mutex<Option<PendingDialogs>> = std::sync::Mutex::new(None);
static NEXT_DIALOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn with_pending<T>(
  action: impl FnOnce(&mut std::collections::HashMap<u64, PendingDialog>) -> T,
) -> T {
  let mut pending = PENDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  action(&mut pending.get_or_insert_with(|| PendingDialogs(Default::default())).0)
}

/// Answer a [`BrowserStatus::Dialog`](crate::BrowserStatus). Must be called on
/// the UI thread. `text` is the value of a prompt and is ignored otherwise.
/// Returns `false` when the dialog is no longer pending.
pub fn answer_dialog(id: u64, accept: bool, text: Option<&str>) -> bool {
  let Some(dialog) = with_pending(|pending| pending.remove(&id)) else {
    return false;
  };
  let text = text.map(CefString::from);
  dialog.callback.cont(accept.into(), text.as_ref());
  true
}

/// A browser's unanswered dialogs, removed from the registry. The caller owns
/// them, so their callbacks are released only after the lock is.
fn take_browser(browser_id: i32) -> Vec<(u64, PendingDialog)> {
  with_pending(|pending| {
    let ids: Vec<u64> = pending
      .iter()
      .filter(|(_, dialog)| dialog.browser_id == browser_id)
      .map(|(id, _)| *id)
      .collect();
    ids
      .into_iter()
      .filter_map(|id| pending.remove(&id).map(|dialog| (id, dialog)))
      .collect()
  })
}

/// A browser that closes takes its unanswered dialogs with it.
pub(crate) fn forget_browser(browser_id: i32) {
  drop(take_browser(browser_id));
}

type Released = Arc<std::sync::Mutex<Vec<PendingDialog>>>;
// SAFETY: created, posted and run on CEF's UI thread only.
struct ReleasedOnUi(Released);
unsafe impl Send for ReleasedOnUi {}
unsafe impl Sync for ReleasedOnUi {}
impl Clone for ReleasedOnUi {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}

wrap_task! {
  struct ReleaseDialogs {
    dialogs: ReleasedOnUi,
  }

  impl Task {
    fn execute(&self) {
      let dialogs = std::mem::take(
        &mut *self.dialogs.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
      );
      drop(dialogs);
    }
  }
}

fn release_later(dialogs: Vec<PendingDialog>) {
  if dialogs.is_empty() {
    return;
  }
  let mut task = ReleaseDialogs::new(ReleasedOnUi(Arc::new(std::sync::Mutex::new(dialogs))));
  post_task(ThreadId::UI, Some(&mut task));
}

wrap_jsdialog_handler! {
  pub struct TauriCefJsDialogHandler {
    status: Arc<crate::browser_status::ScopedStatus>,
  }

  impl JsdialogHandler {
    fn on_jsdialog(
      &self,
      browser: Option<&mut Browser>,
      origin_url: Option<&CefString>,
      dialog_type: JsdialogType,
      message_text: Option<&CefString>,
      default_prompt_text: Option<&CefString>,
      callback: Option<&mut JsdialogCallback>,
      _suppress_message: Option<&mut ::std::os::raw::c_int>,
    ) -> ::std::os::raw::c_int {
      let (Some(browser), Some(callback)) = (browser, callback) else {
        return 0;
      };
      let kind = if dialog_type == JsdialogType::CONFIRM {
        crate::DialogKind::Confirm
      } else if dialog_type == JsdialogType::PROMPT {
        crate::DialogKind::Prompt
      } else {
        crate::DialogKind::Alert
      };
      self.open(
        browser,
        callback,
        kind,
        origin_url.map(ToString::to_string).unwrap_or_default(),
        message_text.map(ToString::to_string).unwrap_or_default(),
        default_prompt_text.map(ToString::to_string).unwrap_or_default(),
      )
    }

    fn on_before_unload_dialog(
      &self,
      browser: Option<&mut Browser>,
      _message_text: Option<&CefString>,
      _is_reload: ::std::os::raw::c_int,
      callback: Option<&mut JsdialogCallback>,
    ) -> ::std::os::raw::c_int {
      let (Some(browser), Some(callback)) = (browser, callback) else {
        return 0;
      };
      // Browsers show their own wording here, never the page's.
      let origin = browser
        .main_frame()
        .map(|frame| CefString::from(&frame.url()).to_string())
        .unwrap_or_default();
      self.open(
        browser,
        callback,
        crate::DialogKind::BeforeUnload,
        origin,
        String::new(),
        String::new(),
      )
    }

    /// The page went away or the dialogs were cancelled; nothing is left to answer.
    fn on_reset_dialog_state(&self, browser: Option<&mut Browser>) {
      let Some(browser_id) = browser.map(|browser| browser.identifier()) else {
        return;
      };
      let dismissed = take_browser(browser_id);
      for (id, _) in &dismissed {
        (self.status)(browser_id, crate::BrowserStatus::DialogDismissed { id: *id });
      }
      // CEF is in the middle of cancelling these very dialogs. Their callbacks
      // are released from a later task, never from inside this notification.
      release_later(dismissed.into_iter().map(|(_, dialog)| dialog).collect());
    }
  }
}

impl TauriCefJsDialogHandler {
  fn open(
    &self,
    browser: &mut Browser,
    callback: &mut JsdialogCallback,
    kind: crate::DialogKind,
    origin: String,
    message: String,
    default_text: String,
  ) -> ::std::os::raw::c_int {
    let browser_id = browser.identifier();
    let id = NEXT_DIALOG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let replaced = with_pending(|pending| {
      pending.insert(
        id,
        PendingDialog {
          browser_id,
          callback: callback.clone(),
        },
      )
    });
    drop(replaced);
    (self.status)(
      browser_id,
      crate::BrowserStatus::Dialog {
        id,
        kind,
        origin,
        message,
        default_text,
      },
    );
    1
  }
}
