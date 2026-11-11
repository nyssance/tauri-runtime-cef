// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::sync::Arc;

use cef::*;

/// Controls of the downloads still in progress. Confined to CEF's UI thread:
/// updates arrive there and [`cancel_download`] requires it.
struct Controls(std::collections::HashMap<u32, DownloadItemCallback>);
// SAFETY: confined to the UI thread, as documented above.
unsafe impl Send for Controls {}
static CONTROLS: std::sync::Mutex<Option<Controls>> = std::sync::Mutex::new(None);

fn with_controls<T>(
  action: impl FnOnce(&mut std::collections::HashMap<u32, DownloadItemCallback>) -> T,
) -> T {
  let mut controls = CONTROLS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  action(&mut controls.get_or_insert_with(|| Controls(Default::default())).0)
}

/// Cancel a download reported through
/// [`BrowserStatus::Download`](crate::BrowserStatus). Must be called on the UI
/// thread. Returns `false` when the download already ended.
pub fn cancel_download(id: u32) -> bool {
  let Some(control) = with_controls(|controls| controls.get(&id).cloned()) else {
    return false;
  };
  control.cancel();
  true
}

wrap_download_handler! {
  pub struct TauriCefDownloadHandler {
    download_handler: Arc<tauri_runtime::webview::DownloadHandler>,
    status: Option<Arc<crate::browser_status::ScopedStatus>>,
  }

  impl DownloadHandler {
    fn can_download(
      &self,
      _browser: Option<&mut Browser>,
      _url: Option<&CefStringUtf16>,
      _request_method: Option<&CefStringUtf16>,
    ) -> ::std::os::raw::c_int {
      // on_before_download is the one that actually validates the download.
      // so we return 1 to allow the download here
      1
    }

    fn on_before_download(
      &self,
      _browser: Option<&mut Browser>,
      download_item: Option<&mut DownloadItem>,
      suggested_name: Option<&CefStringUtf16>,
      callback: Option<&mut BeforeDownloadCallback>,
    ) -> ::std::os::raw::c_int {
      let Some(download_item) = download_item else {
        return 0;
      };
      let Some(callback) = callback else {
        return 0;
      };

      let url_str = CefString::from(&download_item.url()).to_string();
      let Ok(url) = url::Url::parse(&url_str) else {
        return 0;
      };

      let suggested_path = suggested_name
        .map(|s| s.to_string())
        .map(std::path::PathBuf::from)
        .unwrap_or_default();

      let mut destination = suggested_path.clone();

      // Call handler with Requested event.
      let should_allow =
        (self.download_handler)(tauri_runtime::webview::DownloadEvent::Requested {
          url: url.clone(),
          destination: &mut destination,
        });

      if should_allow {
        // Set the download path.
        let destination_cef = CefStringUtf16::from(destination.to_string_lossy().as_ref());

        // If the user callback did not modify the destination, show the dialog.
        let show_dialog = destination == suggested_path;
        callback.cont(Some(&destination_cef), show_dialog as ::std::os::raw::c_int);
      }

      1
    }

    fn on_download_updated(
      &self,
      browser: Option<&mut Browser>,
      download_item: Option<&mut DownloadItem>,
      callback: Option<&mut DownloadItemCallback>,
    ) {
      let Some(download_item) = download_item else {
        return;
      };

      let id = download_item.id();
      let state = if download_item.is_complete() != 0 {
        crate::DownloadState::Complete
      } else if download_item.is_canceled() != 0 {
        crate::DownloadState::Canceled
      } else if download_item.is_interrupted() != 0 {
        crate::DownloadState::Interrupted
      } else {
        crate::DownloadState::InProgress
      };
      // The previous control leaves the lock before it is released.
      let previous = with_controls(|controls| match (state, callback.as_deref()) {
        (crate::DownloadState::InProgress, Some(callback)) => controls.insert(id, callback.clone()),
        (crate::DownloadState::InProgress, None) => None,
        _ => controls.remove(&id),
      });
      drop(previous);
      if let (Some(status), Some(browser)) = (&self.status, browser.as_deref()) {
        status(
          browser.identifier(),
          crate::BrowserStatus::Download(crate::DownloadProgress {
            id,
            url: CefString::from(&download_item.url()).to_string(),
            path: CefString::from(&download_item.full_path()).to_string(),
            received_bytes: download_item.received_bytes(),
            total_bytes: download_item.total_bytes(),
            state,
          }),
        );
      }

      // Get download URL.
      let url_str = CefString::from(&download_item.url()).to_string();
      let Ok(url) = url::Url::parse(&url_str) else {
        return;
      };

      // Check download state - CEF returns i32 where 0 is false, non-zero is true.
      let is_complete = download_item.is_complete() != 0;
      let is_canceled = download_item.is_canceled() != 0;
      let success = is_complete && !is_canceled;

      // Get full path if available - full_path() returns CefStringUserfreeUtf16.
      let full_path = if is_complete || is_canceled {
        let path_cef = download_item.full_path();
        let path_str = CefString::from(&path_cef).to_string();
        if !path_str.is_empty() {
          Some(std::path::PathBuf::from(path_str))
        } else {
          None
        }
      } else {
        None
      };

      // Only call handler when download is finished (complete or canceled).
      if is_complete || is_canceled {
        // Call handler with Finished event.
        (self.download_handler)(tauri_runtime::webview::DownloadEvent::Finished {
          url,
          path: full_path,
          success,
        });
      }
    }
  }
}
