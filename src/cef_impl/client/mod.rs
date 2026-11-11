// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::sync::{Arc, Mutex, Weak, mpsc::Sender};

use cef::*;
use tauri_runtime::{UserEvent, window::WindowId};

use crate::{
  cef_impl::{ipc, request_handler},
  macros::wrap_with_args,
  runtime::{CefRuntime, Message, RuntimeContext},
};

pub(crate) mod auth;
mod command;
mod context_menu;
mod display;
mod download;
mod drag;
mod find;
mod frame;
mod jsdialog;
mod keyboard;
mod life_span;
mod load;
mod permission;
mod process;

use command::{TauriCefCommandHandler, TauriCefCommandHandlerArgs};
use context_menu::TauriCefContextMenuHandler;
use display::{TauriCefDisplayHandler, TauriCefDisplayHandlerArgs};
pub use download::cancel_download;
pub use auth::answer_auth;
pub use jsdialog::answer_dialog;
use download::TauriCefDownloadHandler;
use drag::TauriCefDragHandler;
pub(crate) use drag::{
  DragDropEventTarget, DragDropScriptEvent, DragDropState, WebDragDropResourceRequestHandler,
  WebDragDropResourceRequestHandlerArgs, drag_drop_initialization_script, event_from_script_event,
};
use keyboard::TauriCefKeyboardHandler;
use life_span::{TauriCefChildLifeSpanHandler, TauriCefChildLifeSpanHandlerArgs};
use load::TauriCefLoadHandler;
pub use permission::{PermissionAnswer, answer_permission_request};
pub(crate) use permission::PermissionRequestHandler;
use permission::TauriCefPermissionHandler;
pub(crate) use process::TauriCefBrowserProcessHandler;

pub(crate) struct TauriCefBrowserClientHandlers<T: UserEvent> {
  pub(crate) frame_event_handler: Option<Arc<crate::FrameEventHandler>>,
  pub(crate) ipc_handler: Option<Arc<ipc::IpcHandler<T>>>,
  pub(crate) on_page_load_handler: Option<Arc<tauri_runtime::webview::OnPageLoadHandler>>,
  pub(crate) document_title_changed_handler:
    Option<Arc<tauri_runtime::webview::DocumentTitleChangedHandler>>,
  pub(crate) navigation_handler: Option<Arc<tauri_runtime::webview::NavigationHandler>>,
  pub(crate) new_window_handler:
    Option<Arc<tauri_runtime::webview::NewWindowHandler<T, CefRuntime<T>>>>,
  pub(crate) download_handler: Option<Arc<tauri_runtime::webview::DownloadHandler>>,
  pub(crate) console_message_handler: Option<Arc<crate::ConsoleMessageHandler>>,
  pub(crate) browser_status_handler: Option<Arc<crate::BrowserStatusHandler>>,
  pub(crate) shortcut_filter: Option<Arc<crate::ShortcutFilter>>,
  pub(crate) prompt_permissions: bool,
  pub(crate) prompt_dialogs: bool,
  pub(crate) permission_request_handler: Option<Arc<PermissionRequestHandler>>,
  pub(crate) web_content_process_terminate_handler:
    Option<Arc<tauri_runtime::webview::OnWebContentProcessTerminateHandler>>,
}

impl<T: UserEvent> Clone for TauriCefBrowserClientHandlers<T> {
  fn clone(&self) -> Self {
    Self {
      frame_event_handler: self.frame_event_handler.clone(),
      ipc_handler: self.ipc_handler.clone(),
      on_page_load_handler: self.on_page_load_handler.clone(),
      document_title_changed_handler: self.document_title_changed_handler.clone(),
      navigation_handler: self.navigation_handler.clone(),
      new_window_handler: self.new_window_handler.clone(),
      download_handler: self.download_handler.clone(),
      console_message_handler: self.console_message_handler.clone(),
      browser_status_handler: self.browser_status_handler.clone(),
      shortcut_filter: self.shortcut_filter.clone(),
      prompt_permissions: self.prompt_permissions,
      prompt_dialogs: self.prompt_dialogs,
      permission_request_handler: self.permission_request_handler.clone(),
      web_content_process_terminate_handler: self.web_content_process_terminate_handler.clone(),
    }
  }
}

impl<T: UserEvent> TauriCefBrowserClient<T> {
  /// Chrome state of exactly this client's own native browser.
  fn scoped_status(&self) -> Option<Arc<crate::browser_status::ScopedStatus>> {
    let handler = self.handlers.browser_status_handler.clone()?;
    let state = self.frame_navigation_state.clone();
    Some(Arc::new(move |browser_id, status| {
      if state.has_browser_id(browser_id) {
        handler(status);
      }
    }))
  }
}

wrap_with_args! {
  wrap_client => TauriCefBrowserClientArgs;

  pub(crate) struct TauriCefBrowserClient<T: UserEvent> {
    pub(crate) context: RuntimeContext<T>,
    pub(crate) window_id: WindowId,
    pub(crate) webview_id: u32,
    pub(crate) label: String,
    initial_url: Option<String>,
    is_surface_guest: bool,
    devtools_enabled: bool,
    zoom_hotkeys_enabled: bool,
    allowed_chrome_commands: Vec<crate::ChromeCommandGroup>,
    drag_drop_event_target: DragDropEventTarget,
    drag_drop_handler_enabled: bool,
    drag_drop_state: Arc<Mutex<DragDropState>>,
    frame_navigation_state: crate::FrameNavigationState,
    popup_family: Weak<crate::popup::PopupFamily>,
    opener: Option<crate::popup::PopupRequest>,
    render_handler: Option<cef::RenderHandler>,
    cursor_handler: Option<Arc<crate::CursorHandler>>,
    pub(crate) handlers: TauriCefBrowserClientHandlers<T>,
    sender: Sender<Message<T>>,
  }

  impl Client {
    fn render_handler(&self) -> Option<RenderHandler> {
      self.render_handler.clone()
    }

    fn frame_handler(&self) -> Option<FrameHandler> {
      self.handlers.frame_event_handler.as_ref().map(|handler| {
        frame::TauriCefFrameHandler::new(Some(handler.clone()))
      })
    }

    fn drag_handler(&self) -> Option<DragHandler> {
      self
        .drag_drop_handler_enabled
        .then(|| TauriCefDragHandler::new(self.drag_drop_state.clone()))
    }

    fn request_handler(&self) -> Option<RequestHandler> {
      Some(request_handler::WebRequestHandler::build(
        request_handler::WebRequestHandlerArgs {
          navigation_handler: self.handlers.navigation_handler.clone(),
          frame_event_handler: self.handlers.frame_event_handler.clone(),
          context: self.context.clone(),
          window_id: self.window_id,
          webview_id: self.webview_id,
          drag_drop_event_target: self.drag_drop_event_target,
          drag_drop_handler_enabled: self.drag_drop_handler_enabled,
          drag_drop_state: self.drag_drop_state.clone(),
          web_content_process_terminate_handler:
            self.handlers.web_content_process_terminate_handler.clone(),
          certificate_errors: self.context.certificate_errors,
          // A browser that cannot show CEF's dialogs cannot show its login prompt either.
          auth_prompts: self
            .handlers
            .prompt_dialogs
            .then(|| self.scoped_status())
            .flatten(),
        },
      ))
    }

    fn life_span_handler(&self) -> Option<LifeSpanHandler> {
      let context = self.context.clone();
      let window_id = self.window_id;
      let webview_id = self.webview_id;
      let label = self.label.clone();
      let devtools_enabled = self.devtools_enabled;
      let zoom_hotkeys_enabled = self.zoom_hotkeys_enabled;
      // A CEF-owned popup is a real Chrome window, not an app window, so it keeps the
      // opener's allowances rather than being locked down harder than its opener.
      let allowed_chrome_commands = self.allowed_chrome_commands.clone();
      let target = self.drag_drop_event_target;
      let navigation_handler = self.handlers.navigation_handler.clone();
      let new_window_handler = self.handlers.new_window_handler.clone();
      let download_handler = self.handlers.download_handler.clone();
      let permission_request_handler = self.handlers.permission_request_handler.clone();
      let shortcut_filter = self.handlers.shortcut_filter.clone();
      let prompt_permissions = self.handlers.prompt_permissions;
      let prompt_dialogs = self.handlers.prompt_dialogs;
      let family = self.popup_family.clone();
      let create_popup: Arc<life_span::PopupClientFactory> = Arc::new(move |opener, state| {
        let events = state.clone();
        let title_state = state.clone();
        let title_family = family.clone();
        let status_state = state.clone();
        let status_family = family.clone();
        TauriCefBrowserClient::build(TauriCefBrowserClientArgs {
          context: context.clone(),
          window_id,
          webview_id,
          label: label.clone(),
          initial_url: None,
          is_surface_guest: false,
          devtools_enabled,
          zoom_hotkeys_enabled,
          allowed_chrome_commands: allowed_chrome_commands.clone(),
          drag_drop_event_target: target,
          drag_drop_handler_enabled: false,
          drag_drop_state: Arc::default(),
          frame_navigation_state: state,
          popup_family: family.clone(),
          opener: Some(opener),
          render_handler: None,
          cursor_handler: None,
          handlers: TauriCefBrowserClientHandlers {
            // Only the internal navigation observer, never the opener's app
            // observer. A popup is a separate native browser that navigates
            // wherever its own content goes — an SSO or OAuth window is the
            // standing case — and every `FrameEvent` carries the full URL. An
            // app observes popups without their URLs through `Webview::popups`.
            frame_event_handler: Some(Arc::new(move |event| events.on_frame_event(&event))),
            navigation_handler: navigation_handler.clone(),
            new_window_handler: new_window_handler.clone(),
            // CEF cancels every download of a client whose download handler is
            // NULL, so the popup keeps the opener's — as it did when it still
            // inherited the opener's client outright.
            download_handler: download_handler.clone(),
            // The opener's refusals carry over, its grants do not. A popup shows
            // content the opener navigated to — an SSO or OAuth window is the
            // standing case — and a `PermissionKind` names no origin, so an
            // `Allow` the app gave for its own content is no answer about that
            // other content; CEF's own prompt asks the user instead. A `Deny`
            // carries over so a refused permission cannot be obtained by opening
            // a popup.
            permission_request_handler: permission_request_handler.clone().map(|handler| {
              Arc::new(move |kind| match handler(kind) {
                tauri_runtime::webview::PermissionResponse::Allow => {
                  tauri_runtime::webview::PermissionResponse::Default
                }
                response => response,
              }) as Arc<PermissionRequestHandler>
            }),
            // A `ConsoleMessage` carries the source URL of whatever logged it, so
            // routing a popup's output to the opener's observer would report an
            // SSO or OAuth window's URLs to an observer registered for the app's
            // own content.
            console_message_handler: None,
            // A popup reports its own chrome state through the family's
            // separately identified surface channel, never the opener's.
            browser_status_handler: Some(Arc::new(move |status| {
              if let Some(family) = status_family.upgrade() {
                family.status(&status_state, status);
              }
            })),
            shortcut_filter: shortcut_filter.clone(),
            prompt_permissions,
            prompt_dialogs,
            ipc_handler: None,
            on_page_load_handler: None,
            document_title_changed_handler: Some(Arc::new(move |title| { if let Some(family) = title_family.upgrade() { family.title(&title_state, title); } })),
            web_content_process_terminate_handler: None,
          },
          sender: context.sender.clone(),
        })
      });
      Some(TauriCefChildLifeSpanHandler::build(TauriCefChildLifeSpanHandlerArgs {
        sender: self.sender.clone(),
        window_id: self.window_id,
        webview_id: self.webview_id,
        context: self.context.clone(),
        new_window_handler: self.handlers.new_window_handler.clone(),
        initial_url: self.initial_url.clone(),
        is_surface_guest: self.is_surface_guest,
        frame_navigation_state: self.frame_navigation_state.clone(),
        popup_family: self.popup_family.clone(),
        opener: self.opener.clone(),
        create_popup,
      }))
    }

    fn load_handler(&self) -> Option<LoadHandler> {
      Some(TauriCefLoadHandler::new(
        self.handlers.on_page_load_handler.clone(),
        self.handlers.frame_event_handler.clone(),
        self.scoped_status(),
      ))
    }

    fn display_handler(&self) -> Option<DisplayHandler> {
      Some(TauriCefDisplayHandler::build(TauriCefDisplayHandlerArgs {
        webview_id: self.webview_id,
        context: self.context.clone(),
        document_title_changed_handler: self.handlers.document_title_changed_handler.clone(),
        frame_event_handler: self.handlers.frame_event_handler.clone(),
        console_message_handler: self.handlers.console_message_handler.clone(),
        cursor_handler: self.cursor_handler.clone(),
        frame_navigation_state: self.frame_navigation_state.clone(),
        status: self.scoped_status(),
        is_surface_guest: self.is_surface_guest,
      }))
    }

    fn download_handler(&self) -> Option<DownloadHandler> {
      let status = self.scoped_status();
      self
        .handlers
        .download_handler
        .clone()
        .map(|handler| TauriCefDownloadHandler::new(handler, status))
    }

    fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
      Some(TauriCefContextMenuHandler::new(self.devtools_enabled))
    }

    fn keyboard_handler(&self) -> Option<KeyboardHandler> {
      Some(TauriCefKeyboardHandler::new(
        self.devtools_enabled,
        self.handlers.shortcut_filter.clone(),
        self.scoped_status(),
      ))
    }

    fn command_handler(&self) -> Option<CommandHandler> {
      Some(TauriCefCommandHandler::build(TauriCefCommandHandlerArgs {
        webview_id: self.webview_id,
        context: self.context.clone(),
        devtools_enabled: self.devtools_enabled,
        zoom_hotkeys_enabled: self.zoom_hotkeys_enabled,
        allowed_chrome_commands: self.allowed_chrome_commands.clone(),
        frame_navigation_state: self.frame_navigation_state.clone(),
      }))
    }

    fn permission_handler(&self) -> Option<PermissionHandler> {
      Some(TauriCefPermissionHandler::new(
        self.handlers.permission_request_handler.clone(),
        self
          .handlers
          .prompt_permissions
          .then(|| self.scoped_status())
          .flatten(),
      ))
    }

    fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
      self
        .handlers
        .prompt_dialogs
        .then(|| self.scoped_status())
        .flatten()
        .map(jsdialog::TauriCefJsDialogHandler::new)
    }

    fn find_handler(&self) -> Option<FindHandler> {
      self.scoped_status().map(find::TauriCefFindHandler::new)
    }

    fn on_process_message_received(
      &self,
      browser: Option<&mut Browser>,
      frame: Option<&mut Frame>,
      source_process: ProcessId,
      message: Option<&mut ProcessMessage>,
    ) -> std::os::raw::c_int {
      // A CEF popup (including DevTools) never inherits the root IPC identity.
      if self.opener.is_some() || browser.as_ref().is_none_or(|browser| browser.is_popup() != 0 || !self.frame_navigation_state.has_browser_id(browser.identifier())) { return 0; }
      ipc::on_process_message_received(self, frame, source_process, message)
    }
  }
}
