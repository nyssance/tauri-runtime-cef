// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::sync::Arc;

use cef::*;
use tauri_runtime::UserEvent;

use crate::macros::wrap_with_args;
use crate::runtime::{Message, RuntimeContext};
use crate::webview::INITIAL_LOAD_URL;

#[cfg(target_os = "macos")]
type CefCursorHandle = *mut u8;
#[cfg(not(target_os = "macos"))]
type CefCursorHandle = cef::sys::cef_cursor_handle_t;

wrap_with_args! {
  wrap_display_handler => TauriCefDisplayHandlerArgs;

  pub struct TauriCefDisplayHandler<T: UserEvent> {
    webview_id: u32,
    context: RuntimeContext<T>,
    document_title_changed_handler: Option<Arc<tauri_runtime::webview::DocumentTitleChangedHandler>>,
    frame_event_handler: Option<Arc<crate::FrameEventHandler>>,
    console_message_handler: Option<Arc<crate::ConsoleMessageHandler>>,
    frame_navigation_state: crate::FrameNavigationState,
    cursor_handler: Option<Arc<crate::CursorHandler>>,
    status: Option<Arc<crate::browser_status::ScopedStatus>>,
    is_surface_guest: bool,
  }

  impl DisplayHandler {
    fn on_cursor_change(
      &self,
      browser: Option<&mut Browser>,
      _cursor: CefCursorHandle,
      type_: CursorType,
      info: Option<&CursorInfo>,
    ) -> i32 {
      if let Some(handler) = &self.cursor_handler
        && browser.is_some_and(|browser| {
          self.frame_navigation_state.has_browser_id(browser.identifier())
        })
      {
        handler(type_, info);
        return 1;
      }
      0
    }

    fn on_loading_progress_change(&self, browser: Option<&mut Browser>, progress: f64) {
      if let (Some(status), Some(browser)) = (&self.status, browser) {
        status(browser.identifier(), crate::BrowserStatus::Progress(progress.clamp(0.0, 1.0)));
      }
    }

    fn on_favicon_urlchange(
      &self,
      browser: Option<&mut Browser>,
      icon_urls: Option<&mut CefStringList>,
    ) {
      if let (Some(status), Some(browser)) = (&self.status, browser) {
        let urls = icon_urls
          .map(|urls| std::mem::take(urls).into_iter().collect())
          .unwrap_or_default();
        status(browser.identifier(), crate::BrowserStatus::Favicon(urls));
      }
    }

    fn on_title_change(
      &self,
      _browser: Option<&mut Browser>,
      title: Option<&CefString>,
    ) {
      let Some(handler) = &self.document_title_changed_handler else {
        return;
      };
      let Some(title) = title else {
        return;
      };

      handler(title.to_string());
    }

    fn on_address_change(
      &self,
      browser: Option<&mut Browser>,
      frame: Option<&mut Frame>,
      url: Option<&CefString>,
    ) {
      let Some(url) = url else {
        return;
      };
      let url = url.to_string();

      if url == INITIAL_LOAD_URL {
        return;
      }

      if let (Some(status), Some(browser), Some(frame)) = (&self.status, browser.as_deref(), frame.as_deref())
        && frame.is_main() != 0
      {
        status(browser.identifier(), crate::BrowserStatus::Address(url.clone()));
      }

      if let Ok(url) = url::Url::parse(&url) {
        crate::frame::emit_frame_event(
          &self.frame_event_handler,
          browser,
          frame,
          crate::FrameEventKind::AddressChanged { url },
        );
      }
    }

    fn on_console_message(
      &self,
      browser: Option<&mut Browser>,
      level: LogSeverity,
      message: Option<&CefString>,
      source: Option<&CefString>,
      line: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int {
      if let Some(handler) = &self.console_message_handler {
        // Scoped the way the frame observer is: CEF routes browsers this webview
        // does not own through this very client — a DevTools window is the
        // standing case, and its frontend is itself a page that logs.
        let observed = browser
          .map(|browser| self.frame_navigation_state.has_browser_id(browser.identifier()))
          .unwrap_or(false);
        if observed {
          handler(crate::ConsoleMessage::from_cef(level, message, source, line));
        }
      }

      // 0 leaves CEF's own logging of the message exactly as it was.
      0
    }

    fn on_fullscreen_mode_change(
      &self,
      browser: Option<&mut Browser>,
      fullscreen: ::std::os::raw::c_int,
    ) {
      let Some(browser) = browser else {
        return;
      };
      if let Some(status) = &self.status {
        status(browser.identifier(), crate::BrowserStatus::Fullscreen(fullscreen != 0));
      }
      // Popup clients carry the opener's webview id despite owning a separate browser,
      // and a surface guest has no native view for the window to give the screen to.
      if self.is_surface_guest
        || browser.is_popup() != 0
        || !self.frame_navigation_state.has_browser_id(browser.identifier())
      {
        return;
      }

      let _ = self.context.sender.send(Message::PageFullscreen {
        webview_id: self.webview_id,
        fullscreen: fullscreen != 0,
      });
      self.context.wake_event_loop();
    }
  }
}
