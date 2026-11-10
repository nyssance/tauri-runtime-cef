// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! CEF-owned popup lifetimes preserve the original browser and opener.
//! Surface popups have an explicit, separately identified observation channel;
//! ordinary popup contents never flow to the opener's protocol handlers.
//!
//! A surface popup is an independent browser tab: closing its opener leaves it
//! open, addressable through its own [`SurfacePopup`] handle. Ordinary popups
//! keep following their opener. Window close and application shutdown own every
//! member of a family, whether or not its root is still alive.

use crate::{
  FrameNavigationState, NativeWindowToken,
  webview::{Webview, WebviewSnapshot, add_dev_tools_observer},
};
use cef::*;
use std::sync::{
  Arc, Mutex, Weak,
  atomic::{AtomicBool, Ordering},
};
use tauri_runtime::dpi::{LogicalPosition, LogicalSize, Rect};

/// Opt-in notifications for real popup browsers embedded in the trusted host.
/// Each protocol message identifies its own browser; root observers are unchanged.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum SurfacePopupEvent {
  Created {
    browser_id: i32,
    opener_id: i32,
    token: String,
    url: String,
    /// Addresses this popup for its whole lifetime, independently of its opener.
    popup: SurfacePopup,
  },
  Protocol {
    browser_id: i32,
    message: crate::DevToolsProtocol,
  },
  Closed {
    browser_id: i32,
  },
  Title {
    browser_id: i32,
    title: String,
  },
  /// Native chrome state of the popup's own browser.
  Status {
    browser_id: i32,
    status: crate::BrowserStatus,
  },
}
pub type SurfacePopupHandler = dyn Fn(SurfacePopupEvent) + Send + Sync;

/// Runs a task on the UI thread. Returns `false` once the event loop is gone.
pub(crate) type PopupDispatch = dyn Fn(Box<dyn FnOnce() + Send>) -> bool + Send + Sync;

/// The surface popup no longer exists, or the event loop has shut down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfacePopupClosed;

impl std::fmt::Display for SurfacePopupClosed {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter.write_str("surface popup closed")
  }
}
impl std::error::Error for SurfacePopupClosed {}

/// Handle to one embedded popup browser. It stays usable after the opener and
/// the family root have closed; it never keeps the browser itself alive.
#[derive(Clone)]
pub struct SurfacePopup {
  browser_id: i32,
  family: Weak<PopupFamily>,
  dispatch: Arc<PopupDispatch>,
}

impl std::fmt::Debug for SurfacePopup {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter
      .debug_struct("SurfacePopup")
      .field("browser_id", &self.browser_id)
      .finish_non_exhaustive()
  }
}

impl SurfacePopup {
  /// Native browser identity, as reported by every [`SurfacePopupEvent`].
  pub fn browser_id(&self) -> i32 {
    self.browser_id
  }

  /// Observe the popup on the UI thread. The callback receives `None` when the
  /// popup closed before the callback ran. An error means it will never run.
  pub fn with_webview<F: FnOnce(Option<&Webview>) + Send + 'static>(
    &self,
    callback: F,
  ) -> Result<(), SurfacePopupClosed> {
    let family = self.family.clone();
    let browser_id = self.browser_id;
    if (self.dispatch)(Box::new(move || {
      let popup = family
        .upgrade()
        .and_then(|family| family.observe_popup(browser_id));
      callback(popup.as_ref());
    })) {
      Ok(())
    } else {
      Err(SurfacePopupClosed)
    }
  }
}

const MAX_POPUPS: usize = 128;

#[derive(Clone)]
pub(crate) struct PopupRequest {
  pub(crate) opener: FrameNavigationState,
  popup_id: i32,
  identity: Arc<()>,
}
impl PopupRequest {
  pub(crate) fn is_same(&self, other: &Self) -> bool {
    Arc::ptr_eq(&self.identity, &other.identity)
  }
}

struct Popup {
  surface: bool,
  browser: Browser,
  state: FrameNavigationState,
  opener: FrameNavigationState,
  closing: AtomicBool,
  _observer: Registration,
  dialogs: crate::dialog::DialogState,
}

pub(crate) struct PopupFamily {
  surface_handler: Option<Arc<SurfacePopupHandler>>,
  dispatch: Option<Arc<PopupDispatch>>,
  root: FrameNavigationState,
  closing: AtomicBool,
  popups: Mutex<Vec<Arc<Popup>>>,
  pending: Mutex<Vec<PopupRequest>>,
  windows: Mutex<Vec<(Window, NativeWindowToken)>>,
}

impl PopupFamily {
  pub(crate) fn new(
    root: FrameNavigationState,
    surface_handler: Option<Arc<SurfacePopupHandler>>,
    dispatch: Option<Arc<PopupDispatch>>,
  ) -> Self {
    Self {
      surface_handler,
      dispatch,
      root,
      closing: AtomicBool::new(false),
      popups: Mutex::default(),
      pending: Mutex::default(),
      windows: Mutex::default(),
    }
  }

  pub(crate) fn title(&self, state: &FrameNavigationState, title: String) {
    let id = self.popups.lock().ok().and_then(|popups| {
      popups
        .iter()
        .find(|popup| popup.surface && popup.state.is_same_browser(state))
        .map(|popup| popup.browser.identifier())
    });
    if let Some(browser_id) = id
      && let Some(handler) = &self.surface_handler
    {
      handler(SurfacePopupEvent::Title { browser_id, title });
    }
  }

  pub(crate) fn status(&self, state: &FrameNavigationState, status: crate::BrowserStatus) {
    let id = self.popups.lock().ok().and_then(|popups| {
      popups
        .iter()
        .find(|popup| popup.surface && popup.state.is_same_browser(state))
        .map(|popup| popup.browser.identifier())
    });
    if let Some(browser_id) = id
      && let Some(handler) = &self.surface_handler
    {
      handler(SurfacePopupEvent::Status { browser_id, status });
    }
  }

  pub(crate) fn admits(&self, opener: &FrameNavigationState, browser_id: i32) -> bool {
    // A closed opener keeps its identity, and a detaching family stays live
    // after its root, so the opener's own lifetime is checked explicitly.
    if self.closing.load(Ordering::Acquire)
      || opener.is_closed()
      || !opener.has_browser_id(browser_id)
    {
      return false;
    }
    let Ok(popups) = self.popups.lock() else {
      return false;
    };
    popups.len() < MAX_POPUPS
      && (self.root.is_same_browser(opener)
        || popups.iter().any(|popup| {
          !popup.closing.load(Ordering::Acquire) && popup.state.is_same_browser(opener)
        }))
  }

  pub(crate) fn reserve(
    &self,
    opener: &FrameNavigationState,
    browser_id: i32,
    popup_id: i32,
  ) -> Option<PopupRequest> {
    if !self.admits(opener, browser_id) {
      return None;
    }
    let mut pending = self.pending.lock().ok()?;
    if pending.len() + self.popups.lock().ok()?.len() >= MAX_POPUPS
      || pending
        .iter()
        .any(|request| request.popup_id == popup_id && request.opener.is_same_browser(opener))
    {
      return None;
    }
    let request = PopupRequest {
      opener: opener.clone(),
      popup_id,
      identity: Arc::new(()),
    };
    pending.push(request.clone());
    Some(request)
  }

  pub(crate) fn abort(&self, opener: &FrameNavigationState, popup_id: i32) -> Option<PopupRequest> {
    let mut pending = self.pending.lock().ok()?;
    let index = pending
      .iter()
      .position(|request| request.popup_id == popup_id && request.opener.is_same_browser(opener))?;
    Some(pending.remove(index))
  }

  /// Surface popups of an observed family outlive their opener.
  fn detaches(&self, popup: &Popup) -> bool {
    popup.surface && self.surface_handler.is_some() && self.dispatch.is_some()
  }

  /// Whether a reservation can still resolve into a popup.
  pub(crate) fn holds(&self, request: &PopupRequest) -> bool {
    !self.closing.load(Ordering::Acquire) && !request.opener.is_closed()
  }

  fn observe_popup(&self, browser_id: i32) -> Option<Webview> {
    self
      .observe()
      .into_iter()
      .find(|popup| popup.snapshot().browser_id == browser_id)
  }

  pub(crate) fn created(
    self: &Arc<Self>,
    browser: &Browser,
    request: &PopupRequest,
    state: &FrameNavigationState,
  ) {
    let reserved = self
      .pending
      .lock()
      .map(|mut pending| {
        let found = pending.iter().any(|entry| entry.is_same(request));
        pending.retain(|entry| !entry.is_same(request));
        found
      })
      .unwrap_or(false);
    let opener = &request.opener;
    let Some(host) = browser.host() else {
      return;
    };
    if !reserved || !self.admits(opener, host.opener_identifier()) {
      state.close();
      host.close_browser(1);
      return;
    }
    let dialogs = crate::dialog::DialogState::new(state.clone());
    // Popup protocol remains separate from root observers. Only an explicit
    // surface callback receives messages, tagged with the native browser ID.
    let token = CefString::from(&host.surface_embed_token()).to_string();
    let browser_id = browser.identifier();
    let handlers: Arc<Mutex<Vec<Arc<crate::webview::DevToolsProtocolHandler>>>> = Arc::default();
    if !token.is_empty()
      && let Some(handler) = self.surface_handler.clone()
    {
      handlers.lock().unwrap().push(Arc::new(move |message| {
        handler(SurfacePopupEvent::Protocol {
          browser_id,
          message,
        })
      }));
    }
    let Some(observer) = add_dev_tools_observer(browser, handlers, Arc::default(), dialogs.clone())
    else {
      state.close();
      host.close_browser(1);
      return;
    };
    let surface = !token.is_empty();
    let popup = Arc::new(Popup {
      surface,
      browser: browser.clone(),
      state: state.clone(),
      opener: opener.clone(),
      closing: AtomicBool::new(false),
      _observer: observer,
      dialogs,
    });
    if let Ok(mut popups) = self.popups.lock() {
      popups.push(popup);
      drop(popups);
      if surface
        && let Some(handler) = &self.surface_handler
        && let Some(dispatch) = &self.dispatch
      {
        let url = browser
          .main_frame()
          .map(|frame| CefString::from(&frame.url()).to_string())
          .unwrap_or_default();
        handler(SurfacePopupEvent::Created {
          browser_id,
          opener_id: host.opener_identifier(),
          token,
          url,
          popup: SurfacePopup {
            browser_id,
            family: Arc::downgrade(self),
            dispatch: dispatch.clone(),
          },
        });
      }
    } else {
      state.close();
      host.close_browser(1);
    }
  }

  /// Revoke dependent descendants before asking CEF to close them. The native
  /// close callbacks remove their browser references; no family lock crosses CEF.
  /// Detached surface popups survive, together with everything they opened.
  pub(crate) fn closed(&self, state: &FrameNavigationState, browser_id: i32) {
    if !state.has_browser_id(browser_id) {
      return;
    }
    let surface = self
      .popups
      .lock()
      .map(|popups| {
        popups
          .iter()
          .any(|popup| popup.surface && popup.state.is_same_browser(state))
      })
      .unwrap_or(false);
    if surface && let Some(handler) = &self.surface_handler {
      handler(SurfacePopupEvent::Closed { browser_id });
    }
    state.close();
    let root_closed = self.root.is_same_browser(state);
    // A family that detaches its surface popups stays live after its root:
    // survivors keep opening popups and remain observable. Teardown still
    // revokes it through `close_all`.
    let family_closed = root_closed && self.dispatch.is_none();
    if family_closed {
      self.closing.store(true, Ordering::Release);
    }
    let descendants = {
      let Ok(mut popups) = self.popups.lock() else {
        return;
      };
      popups.retain(|popup| !popup.state.is_same_browser(state));
      revoke_descendants(
        state,
        family_closed,
        popups.iter().enumerate().map(|(index, popup)| {
          (
            index,
            &popup.state,
            &popup.opener,
            &popup.closing,
            self.detaches(popup),
          )
        }),
      )
      .into_iter()
      .map(|index| Arc::clone(&popups[index]))
      .collect::<Vec<_>>()
    };
    for popup in descendants {
      if popup.browser.is_valid() != 0
        && let Some(host) = popup.browser.host()
      {
        host.close_browser(1);
      }
    }
  }

  /// Window close and app shutdown own the whole family, so teardown never
  /// depends on the root having observed a native browser identity. `closed`
  /// keeps its exact-identity guard for the per-browser native callback; a root
  /// that was exhausted before its first frame event would otherwise leave every
  /// popup open and the event loop waiting on them forever.
  pub(crate) fn close_all(&self) {
    self.closing.store(true, Ordering::Release);
    self.root.close();
    let descendants = {
      let Ok(popups) = self.popups.lock() else {
        return;
      };
      revoke_descendants(
        &self.root,
        true,
        popups
          .iter()
          .enumerate()
          .map(|(index, popup)| (index, &popup.state, &popup.opener, &popup.closing, false)),
      )
      .into_iter()
      .map(|index| Arc::clone(&popups[index]))
      .collect::<Vec<_>>()
    };
    for popup in descendants {
      if popup.browser.is_valid() != 0
        && let Some(host) = popup.browser.host()
      {
        host.close_browser(1);
      }
    }
  }

  /// A revoked family admits no further popup, so nothing it still has
  /// reserved can ever be created. CEF does not always report the abort of a
  /// popup it discards, so the reservations of a revoked family are what the
  /// event loop would otherwise wait on forever.
  #[cfg(test)]
  pub(crate) fn is_revoked(&self) -> bool {
    self.closing.load(Ordering::Acquire)
  }

  pub(crate) fn observe(&self) -> Vec<Webview> {
    if self.closing.load(Ordering::Acquire) {
      return Vec::new();
    }
    let popups = self
      .popups
      .lock()
      .map(|popups| popups.clone())
      .unwrap_or_default();
    // Several popup browser tabs may share one CEF window. Window identity is
    // owned by the family, never minted independently for each tab.
    let mut windows = self
      .windows
      .lock()
      .map(|windows| windows.clone())
      .unwrap_or_default();
    windows.retain(|(window, _)| window.is_valid() != 0 && window.is_closed() == 0);
    let observations = popups
      .iter()
      .filter_map(|popup| {
        if popup.closing.load(Ordering::Acquire) || popup.browser.is_valid() == 0 {
          return None;
        }
        let mut browser = popup.browser.clone();
        let view =
          browser_view_get_for_browser(Some(&mut browser)).filter(|view| view.is_valid() != 0);
        let observed_window = view
          .as_ref()
          .and_then(ImplView::window)
          .filter(|window| window.is_valid() != 0 && window.is_closed() == 0);
        let window = observed_window.as_ref().and_then(|window| {
          if let Some((_, token)) = windows
            .iter()
            .find(|(previous, _)| window.is_same(Some(&mut View::from(previous))) != 0)
          {
            return Some(token.clone());
          }
          if windows.len() >= MAX_POPUPS {
            return None;
          }
          let token = NativeWindowToken::new();
          windows.push((window.clone(), token.clone()));
          Some(token)
        });
        let bounds = view.as_ref().map(|view| {
          let rect = view.bounds();
          Rect {
            position: LogicalPosition::new(rect.x, rect.y).into(),
            size: LogicalSize::new(rect.width, rect.height).into(),
          }
        });
        let visible = view
          .as_ref()
          .zip(observed_window.as_ref())
          .map(|(view, window)| {
            view.is_drawn() != 0 && window.is_visible() != 0 && window.is_minimized() == 0
          });
        let document = popup.state.observe_document(&browser);
        let dialogs = popup.dialogs.snapshot(document.as_ref());
        let snapshot = WebviewSnapshot {
          browser_id: browser.identifier(),
          dialogs,
          document,
          window_label: None,
          window,
          // CEF owns a popup's native window and the runtime never reparents
          // it, so there is no independently observed parent to check the view
          // against: the reported window is the one the view itself named.
          // The relationship is therefore never established here, and reporting
          // a match would hand a caller gating a native effect on `Some(true)`
          // an assertion nothing verified.
          parent_matches: None,
          bounds,
          visible,
        };
        let mut native = Webview::new(browser, snapshot, popup.state.clone());
        native.set_opener(popup.opener.clone());
        Some(native)
      })
      .collect();
    if let Ok(mut retained) = self.windows.lock() {
      *retained = windows;
    }
    observations
  }
}

/// The graph walk is independent of CEF calls. Revoke every dependent descendant
/// before returning handles for native close, including when callbacks arrive
/// out of order. A detached node is revoked only when the whole family is.
fn revoke_descendants<'a>(
  state: &FrameNavigationState,
  family_closed: bool,
  nodes: impl Iterator<
    Item = (
      usize,
      &'a FrameNavigationState,
      &'a FrameNavigationState,
      &'a AtomicBool,
      bool,
    ),
  > + Clone,
) -> Vec<usize> {
  let mut revoked = vec![state.clone()];
  let mut indices = Vec::new();
  loop {
    let before = indices.len();
    for (index, state, opener, closing, detached) in nodes.clone() {
      if !closing.load(Ordering::Acquire)
        && (family_closed
          || (!detached && revoked.iter().any(|parent| parent.is_same_browser(opener))))
      {
        closing.store(true, Ordering::Release);
        state.close();
        revoked.push(state.clone());
        indices.push(index);
      }
    }
    if indices.len() == before {
      return indices;
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  fn created(id: i32) -> FrameNavigationState {
    let state = FrameNavigationState::new();
    state.on_frame_event(&crate::FrameEvent {
      browser_id: id,
      frame_id: "main".into(),
      is_main: true,
      kind: crate::FrameEventKind::Created,
    });
    state
  }
  #[test]
  fn closing_an_opener_revokes_only_its_exact_descendants() {
    let root = created(1);
    let first = created(2);
    let nested = created(3);
    let sibling = created(4);
    // Deliberately put a descendant before its opener.
    let nodes = [
      (nested.clone(), first.clone(), AtomicBool::new(false)),
      (first.clone(), root.clone(), AtomicBool::new(false)),
      (sibling.clone(), root.clone(), AtomicBool::new(false)),
    ];
    let refs = || {
      nodes
        .iter()
        .enumerate()
        .map(|(index, (state, opener, closing))| (index, state, opener, closing, false))
    };
    assert!(revoke_descendants(&created(2), false, refs()).is_empty());
    assert_eq!(revoke_descendants(&first, false, refs()), vec![0]);
    assert!(nodes[0].2.load(Ordering::Acquire));
    assert!(!nodes[2].2.load(Ordering::Acquire));
    assert_eq!(revoke_descendants(&root, true, refs()), vec![1, 2]);
    assert!(nodes.iter().all(|node| node.2.load(Ordering::Acquire)));
    assert!(revoke_descendants(&root, true, refs()).is_empty());
  }
  #[test]
  fn detached_popups_survive_their_opener_but_not_family_teardown() {
    let root = created(1);
    let tab = created(2);
    let nested_tab = created(3);
    let dependent = created(4);
    let nested_dependent = created(5);
    let nodes = [
      (tab.clone(), root.clone(), AtomicBool::new(false), true),
      (
        nested_tab.clone(),
        tab.clone(),
        AtomicBool::new(false),
        true,
      ),
      (
        dependent.clone(),
        root.clone(),
        AtomicBool::new(false),
        false,
      ),
      // An ordinary popup still follows the detached tab that opened it.
      (
        nested_dependent.clone(),
        tab.clone(),
        AtomicBool::new(false),
        false,
      ),
    ];
    let refs = || {
      nodes
        .iter()
        .enumerate()
        .map(|(index, (state, opener, closing, detached))| {
          (index, state, opener, closing, *detached)
        })
    };
    // The opener closes: only its ordinary popup goes with it.
    assert_eq!(revoke_descendants(&root, false, refs()), vec![2]);
    assert!(!tab.is_closed() && !nested_tab.is_closed());
    // An intermediate tab closes: its nested tab survives.
    assert_eq!(revoke_descendants(&tab, false, refs()), vec![3]);
    assert!(!nested_tab.is_closed());
    // Window close and shutdown take every survivor.
    assert_eq!(revoke_descendants(&root, true, refs()), vec![0, 1]);
    assert!(nodes.iter().all(|node| node.2.load(Ordering::Acquire)));
  }
  #[test]
  fn a_detaching_family_outlives_its_root_until_teardown() {
    let root = created(1);
    let dispatch: Arc<PopupDispatch> = Arc::new(|task| {
      task();
      true
    });
    let family = Arc::new(PopupFamily::new(
      root.clone(),
      Some(Arc::new(|_| {})),
      Some(dispatch),
    ));
    let from_root = family.reserve(&root, 1, 7).unwrap();
    family.closed(&root, 1);
    assert!(!family.is_revoked());
    // The root can no longer open anything, so its reservation is dead and
    // must not keep the event loop waiting.
    assert!(!family.holds(&from_root));
    assert!(!family.admits(&root, 1));
    family.close_all();
    assert!(family.is_revoked());
  }
  #[test]
  fn a_handle_to_a_closed_popup_reports_absence_instead_of_another_browser() {
    let dispatch: Arc<PopupDispatch> = Arc::new(|task| {
      task();
      true
    });
    let family = Arc::new(PopupFamily::new(
      created(1),
      Some(Arc::new(|_| {})),
      Some(dispatch.clone()),
    ));
    let popup = SurfacePopup {
      browser_id: 2,
      family: Arc::downgrade(&family),
      dispatch,
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    let observed = sender.clone();
    popup
      .with_webview(move |native| observed.send(native.is_some()).unwrap())
      .unwrap();
    assert_eq!(receiver.try_recv(), Ok(false));
    drop(family);
    popup
      .with_webview(move |native| sender.send(native.is_some()).unwrap())
      .unwrap();
    assert_eq!(receiver.try_recv(), Ok(false));
    let stopped = SurfacePopup {
      browser_id: 2,
      family: Weak::new(),
      dispatch: Arc::new(|_| false),
    };
    assert_eq!(stopped.with_webview(|_| {}), Err(SurfacePopupClosed));
  }
  #[test]
  fn pending_popups_are_bounded_and_retained_until_exact_creation_or_abort() {
    let root = created(1);
    let family = PopupFamily::new(root.clone(), None, None);
    let first = family.reserve(&root, 1, 7).unwrap();
    assert!(family.reserve(&root, 1, 7).is_none());
    assert!(family.abort(&created(1), 7).is_none());
    for id in 8..(7 + MAX_POPUPS as i32) {
      assert!(family.reserve(&root, 1, id).is_some());
    }
    assert!(family.reserve(&root, 1, 1000).is_none());
    family.closed(&root, 1);
    assert_eq!(family.pending.lock().unwrap().len(), MAX_POPUPS);
    assert!(family.abort(&root, 7).unwrap().is_same(&first));
    assert!(family.abort(&root, 7).is_none());
    assert!(family.reserve(&root, 1, 1000).is_none());
  }
  #[test]
  fn teardown_revokes_a_family_still_holding_an_unresolved_reservation() {
    let root = created(1);
    let family = PopupFamily::new(root.clone(), None, None);
    // CEF discarded this popup without ever creating or aborting it.
    let request = family.reserve(&root, 1, 7).unwrap();
    assert!(!family.is_revoked());
    // A popup's own native close leaves the family live, so the event loop
    // keeps waiting for the reservations that can still be created.
    family.closed(&created(2), 2);
    assert!(!family.is_revoked());
    assert_eq!(family.pending.lock().unwrap().len(), 1);
    // Window close and app shutdown own the family outright: once revoked, no
    // reservation of it can ever be created, so none may hold the loop open.
    family.close_all();
    assert!(family.is_revoked());
    assert!(!family.admits(&request.opener, 1));
  }
  #[test]
  fn popup_admission_requires_the_live_exact_native_opener() {
    let root = created(1);
    let family = PopupFamily::new(root.clone(), None, None);
    assert!(family.admits(&root, 1));
    assert!(!family.admits(&root, 2));
    assert!(!family.admits(&created(1), 1));
    assert!(!family.admits(&FrameNavigationState::new(), 1));
    family.closed(&root, 2);
    assert!(family.admits(&root, 1));
    family.closed(&root, 1);
    assert!(!family.admits(&root, 1));
    assert!(family.observe().is_empty());
  }
  #[test]
  fn teardown_closes_a_family_whose_root_never_observed_a_browser_id() {
    // A root that never observed a native frame event has no browser identity.
    let root = FrameNavigationState::new();
    assert!(!root.has_browser_id(1));
    let family = PopupFamily::new(root.clone(), None, None);
    // The per-browser native callback cannot match an unobserved identity.
    family.closed(&root, 1);
    assert!(!family.closing.load(Ordering::Acquire));
    // Window close and app shutdown own the family outright and must not.
    family.close_all();
    assert!(family.closing.load(Ordering::Acquire));
    assert!(family.observe().is_empty());
  }
}
