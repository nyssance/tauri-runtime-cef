// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The application's permission policy, as CEF asks for it.
//!
//! CEF has two entry points and they behave very differently once answered. The
//! user-facing half of what follows is documented on
//! [`CefWebviewAttributes`](crate::CefWebviewAttributes).
//!
//! # The prompt path answers once per origin, forever
//!
//! `on_show_permission_prompt` is reached only while Chromium's stored content
//! setting for that (origin, permission) still says "ask", and `cont` persists the
//! answer into the on-disk profile: `ACCEPT` through
//! `PermissionRequestManager::Accept()`, the path a click on Chrome's Allow button
//! takes, and `DENY` through `Deny()`, which stores BLOCK. Either way Chromium
//! never asks again, so a handler whose answer depends on application state is
//! ignored from its second request onwards, including across restarts.
//!
//! There is no callback for "the app changed its mind". An app that has to revoke
//! a grant, or undo a refusal, rewrites the content setting itself:
//! `Webview::browser()` reaches the `cef::Browser`, and from it
//! `host().request_context().set_content_setting(...)`.
//!
//! # The media path answers every call
//!
//! Chromium routes every `getUserMedia()` call through
//! `on_request_media_access_permission`, so camera and microphone requests do
//! reach the handler each time and a changing answer is honored.
//! `MediaAccessCallback` persists nothing, which is why this path — and only this
//! path — writes the content settings itself (see [`allow_content_settings`]).
//!
//! # Unmapped request types are [`PermissionKind::Other`]
//!
//! [`PERMISSION_KINDS`] is a partial map: Chromium has more request types than
//! Tauri has kinds, and everything left over — storage access, FedCM, protocol
//! handler registration, idle detection, local and loopback network access, web
//! app installation, the WebXR sessions, hand tracking, keyboard lock and disk
//! quota — arrives as `PermissionKind::Other`, as does any request type a future
//! CEF build adds.
//!
//! Failing closed is deliberate, but it means a handler written for another
//! platform as `match kind { Camera => Allow, _ => Deny }` hard-denies all of
//! them, and denying storage access or FedCM breaks third-party SSO flows
//! outright. A handler that only means to answer about the kinds it names should
//! return [`PermissionResponse::Default`] for the rest.

use std::sync::Arc;

use cef::sys::cef_media_access_permission_types_t as MediaPermissionType;
use cef::sys::cef_permission_request_types_t as PermissionType;
use cef::*;
use tauri_runtime::webview::{PermissionKind, PermissionResponse};

/// The application's answer to a permission request, as
/// `PendingWebview::permission_request_handler` carries it.
///
/// `tauri_runtime::webview` declares the same alias but keeps it private, so it is
/// redeclared here; both name one and the same type.
pub(crate) type PermissionRequestHandler =
  dyn Fn(PermissionKind) -> PermissionResponse + Send + Sync;

const AUDIO_CAPTURE: u32 = MediaPermissionType::CEF_MEDIA_PERMISSION_DEVICE_AUDIO_CAPTURE as u32;
const VIDEO_CAPTURE: u32 = MediaPermissionType::CEF_MEDIA_PERMISSION_DEVICE_VIDEO_CAPTURE as u32;
const DESKTOP_AUDIO_CAPTURE: u32 =
  MediaPermissionType::CEF_MEDIA_PERMISSION_DESKTOP_AUDIO_CAPTURE as u32;
const DESKTOP_VIDEO_CAPTURE: u32 =
  MediaPermissionType::CEF_MEDIA_PERMISSION_DESKTOP_VIDEO_CAPTURE as u32;

/// The bits a `getDisplayMedia()` request sets. Never granted from here; see
/// [`grants_desktop_capture`].
const DESKTOP_CAPTURE: u32 = DESKTOP_AUDIO_CAPTURE | DESKTOP_VIDEO_CAPTURE;

/// Media capture permissions granted to Alloy style browsers. Desktop capture is
/// deliberately excluded.
const ALLOY_MEDIA_PERMISSIONS: u32 = AUDIO_CAPTURE | VIDEO_CAPTURE;

/// The [`PermissionKind`] the application is asked about for each permission request
/// type.
///
/// Chromium's request types are finer grained than Tauri's kinds — the plain camera
/// stream and its pan-tilt-zoom control are both `Camera` — and most of them have no
/// Tauri counterpart at all. A request type absent from this table is reported as
/// [`PermissionKind::Other`]; see the module docs.
///
/// [`PermissionKind::Autoplay`] has no entry: Chromium gates autoplay through its
/// media engagement policy rather than through a permission request, so no CEF
/// request carries it.
const PERMISSION_KINDS: &[(u32, PermissionKind)] = &[
  (
    PermissionType::CEF_PERMISSION_TYPE_CAMERA_PAN_TILT_ZOOM as u32,
    PermissionKind::Camera,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_CAMERA_STREAM as u32,
    PermissionKind::Camera,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_MIC_STREAM as u32,
    PermissionKind::Microphone,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_CAPTURED_SURFACE_CONTROL as u32,
    PermissionKind::DisplayCapture,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_GEOLOCATION as u32,
    PermissionKind::Geolocation,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_NOTIFICATIONS as u32,
    PermissionKind::Notifications,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_CLIPBOARD as u32,
    PermissionKind::ClipboardRead,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_MIDI_SYSEX as u32,
    PermissionKind::Midi,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_SENSORS as u32,
    PermissionKind::Sensors,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_LOCAL_FONTS as u32,
    PermissionKind::LocalFonts,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_WINDOW_MANAGEMENT as u32,
    PermissionKind::WindowManagement,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_POINTER_LOCK as u32,
    PermissionKind::PointerLock,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_MULTIPLE_DOWNLOADS as u32,
    PermissionKind::AutomaticDownloads,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_FILE_SYSTEM_ACCESS as u32,
    PermissionKind::FileSystemAccess,
  ),
  (
    PermissionType::CEF_PERMISSION_TYPE_PROTECTED_MEDIA_IDENTIFIER as u32,
    PermissionKind::MediaKeySystemAccess,
  ),
];

/// The [`PermissionKind`] the application is asked about for each media access type.
///
/// Both desktop capture types are one kind: `getDisplayMedia` is a single Tauri
/// permission whether the page asks for the screen's video, its audio, or both.
const MEDIA_PERMISSION_KINDS: &[(u32, PermissionKind)] = &[
  (AUDIO_CAPTURE, PermissionKind::Microphone),
  (VIDEO_CAPTURE, PermissionKind::Camera),
  (DESKTOP_AUDIO_CAPTURE, PermissionKind::DisplayCapture),
  (DESKTOP_VIDEO_CAPTURE, PermissionKind::DisplayCapture),
];

/// How the application answered a whole CEF permission request.
///
/// CEF asks about a bitmask of types and is answered once for all of them, so the
/// per-kind answers have to be combined. [`Self::Denied`] wins over everything,
/// because a refusal must never end up granting the rest of the request, and a
/// request is granted only when the application allowed every type in it. Anything
/// else leaves part of the request unanswered, and the platform default runs for it
/// unchanged.
enum AppDecision {
  /// The application denied at least one of the requested permissions.
  Denied,
  /// The application allowed every requested permission.
  Allowed,
  /// The application left at least one requested permission to the platform, and
  /// denied none. Also the answer when the webview has no permission handler.
  NoOpinion,
}

/// The user's answer to a permission request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionAnswer {
  /// Granted, and remembered for the origin.
  Allow,
  /// Refused, and remembered for the origin.
  Deny,
  /// Left unanswered: nothing is granted and nothing is remembered, so the
  /// origin may ask again.
  Dismiss,
}

/// A request the embedder was asked about and has not answered yet.
enum PendingAnswer {
  Prompt(PermissionPromptCallback),
  Media {
    callback: MediaAccessCallback,
    browser: Browser,
    origin: Option<String>,
    requested: u32,
  },
}

struct PendingRequest {
  browser_id: i32,
  /// CEF's own identifier, for the prompt path's dismissal callback.
  prompt_id: Option<u64>,
  answer: PendingAnswer,
}

/// Only ever touched on CEF's UI thread: requests arrive there, and
/// [`answer_permission_request`] requires it.
struct PendingRequests(std::collections::HashMap<u64, PendingRequest>);
// SAFETY: the map is confined to the UI thread, as documented above. The mutex
// exists to satisfy `static`, never to share the callbacks across threads.
unsafe impl Send for PendingRequests {}

static PENDING: std::sync::Mutex<Option<PendingRequests>> = std::sync::Mutex::new(None);
static NEXT_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn with_pending<T>(action: impl FnOnce(&mut std::collections::HashMap<u64, PendingRequest>) -> T) -> T {
  let mut pending = PENDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  action(&mut pending.get_or_insert_with(|| PendingRequests(Default::default())).0)
}

fn remember(request: PendingRequest) -> u64 {
  let id = NEXT_REQUEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
  // Nothing CEF-owned is released while the registry is locked.
  let replaced = with_pending(|pending| pending.insert(id, request));
  drop(replaced);
  id
}

/// Answer a [`BrowserStatus::PermissionRequest`](crate::BrowserStatus). Must be
/// called on the UI thread. Returns `false` when the request is no longer
/// pending.
///
/// An accepted prompt is persisted for its origin by Chromium, exactly as a
/// click on a browser's own Allow button. Camera and microphone are granted for
/// the request and recorded as content settings; the desktop is never granted
/// from here.
pub fn answer_permission_request(id: u64, answer: PermissionAnswer) -> bool {
  let Some(request) = with_pending(|pending| pending.remove(&id)) else {
    return false;
  };
  let allow = answer == PermissionAnswer::Allow;
  match request.answer {
    PendingAnswer::Prompt(callback) => callback.cont(match answer {
      PermissionAnswer::Allow => PermissionRequestResult::ACCEPT,
      PermissionAnswer::Deny => PermissionRequestResult::DENY,
      PermissionAnswer::Dismiss => PermissionRequestResult::DISMISS,
    }),
    PendingAnswer::Media {
      callback,
      browser,
      origin,
      requested,
    } => {
      let allowed = if allow {
        requested & ALLOY_MEDIA_PERMISSIONS
      } else {
        0
      };
      if allowed != 0 {
        let origin = origin.as_deref().map(CefString::from);
        allow_content_settings(
          browser.host().as_ref(),
          origin.as_ref(),
          &media_content_settings(allowed),
        );
      }
      callback.cont(allowed);
    }
  }
  true
}

/// A browser that closes takes its unanswered requests with it.
pub(crate) fn forget_browser(browser_id: i32) {
  let dropped = with_pending(|pending| {
    let ids: Vec<u64> = pending
      .iter()
      .filter(|(_, request)| request.browser_id == browser_id)
      .map(|(id, _)| *id)
      .collect();
    ids
      .into_iter()
      .filter_map(|id| pending.remove(&id))
      .collect::<Vec<_>>()
  });
  for request in dropped {
    if let PendingAnswer::Media { callback, .. } = request.answer {
      callback.cancel();
    }
  }
}

fn kinds(requested: u32, kind: fn(u32) -> PermissionKind) -> Vec<PermissionKind> {
  let mut kinds = Vec::new();
  for permission in requested_permissions(requested) {
    let kind = kind(permission);
    if !kinds.contains(&kind) {
      kinds.push(kind);
    }
  }
  kinds
}

wrap_permission_handler! {
  pub struct TauriCefPermissionHandler {
    permission_request_handler: Option<Arc<PermissionRequestHandler>>,
    // When set, a request the application left unanswered is offered to the
    // embedder's status observer instead of the runtime style's default.
    prompts: Option<Arc<crate::browser_status::ScopedStatus>>,
  }

  impl PermissionHandler {
    fn on_request_media_access_permission(
      &self,
      browser: Option<&mut Browser>,
      _frame: Option<&mut Frame>,
      requesting_origin: Option<&CefString>,
      requested_permissions: u32,
      callback: Option<&mut MediaAccessCallback>,
    ) -> ::std::os::raw::c_int {
      let host = browser_host(browser);

      match self.decide(requested_permissions, media_permission_kind) {
        // Answer for the application, whatever the runtime style: neither Chrome's
        // prompt nor Alloy's blanket grant may override an explicit answer.
        AppDecision::Denied => {
          let Some(callback) = callback else {
            return 0;
          };
          callback.cont(0);
          1
        }
        AppDecision::Allowed => {
          // An `Allow` grants the camera and the microphone, but never the desktop:
          // no `PermissionKind` answer can name what a screen share would expose,
          // so that choice stays with CEF's picker. See `grants_desktop_capture`.
          if grants_desktop_capture(requested_permissions) {
            return 0;
          }

          let Some(callback) = callback else {
            return 0;
          };
          allow_content_settings(
            host.as_ref(),
            requesting_origin,
            &media_content_settings(requested_permissions),
          );
          callback.cont(requested_permissions);
          1
        }
        AppDecision::NoOpinion => {
          // The embedder prompts: nothing is granted until it answers. A desktop
          // capture request is never the embedder's to grant; see below.
          if let (Some(prompts), Some(browser), Some(callback)) =
            (&self.prompts, host.as_ref().and_then(|host| host.browser()), callback.as_deref())
            && !grants_desktop_capture(requested_permissions)
            && requested_permissions & ALLOY_MEDIA_PERMISSIONS != 0
          {
            let browser_id = browser.identifier();
            let origin = requesting_origin.map(ToString::to_string);
            let id = remember(PendingRequest {
              browser_id,
              prompt_id: None,
              answer: PendingAnswer::Media {
                callback: callback.clone(),
                browser,
                origin: origin.clone(),
                requested: requested_permissions,
              },
            });
            prompts(
              browser_id,
              crate::BrowserStatus::PermissionRequest {
                id,
                origin: origin.unwrap_or_default(),
                kinds: kinds(requested_permissions, media_permission_kind),
              },
            );
            return 1;
          }

          // Chrome style displays the permission request UI and records the outcome as a
          // content setting. That content setting is what keeps `enumerateDevices()` from
          // returning a redacted device list, so let CEF handle the request.
          if !is_alloy_style(host.as_ref()) {
            return 0;
          }

          // Alloy style has no permission UI and its default handling denies the request,
          // so grant camera and microphone capture here. Desktop capture is left to that
          // default handling, which refuses it — see `grants_desktop_capture`.
          if grants_desktop_capture(requested_permissions) {
            return 0;
          }

          let Some(callback) = callback else {
            return 0;
          };

          let allowed = requested_permissions & ALLOY_MEDIA_PERMISSIONS;
          if allowed == 0 {
            return 0;
          }

          allow_content_settings(
            host.as_ref(),
            requesting_origin,
            &media_content_settings(allowed),
          );

          callback.cont(allowed);
          1
        }
      }
    }

    fn on_show_permission_prompt(
      &self,
      browser: Option<&mut Browser>,
      prompt_id: u64,
      requesting_origin: Option<&CefString>,
      requested_permissions: u32,
      callback: Option<&mut PermissionPromptCallback>,
    ) -> ::std::os::raw::c_int {
      let host = browser_host(browser);

      match self.decide(requested_permissions, permission_kind) {
        // Answer for the application, whatever the runtime style, and without
        // showing Chrome's prompt: the application already decided.
        AppDecision::Denied => {
          let Some(callback) = callback else {
            return 0;
          };
          callback.cont(PermissionRequestResult::DENY);
          1
        }
        AppDecision::Allowed => {
          let Some(callback) = callback else {
            return 0;
          };
          // No content setting is written here: `cont(ACCEPT)` reaches
          // `PermissionRequestManager::Accept()`, which persists the grant itself.
          // Writing one on top would also be over-broad — it names no top-level
          // URL, so a storage-access grant Chromium scopes to an (embedded origin,
          // top-level site) pair would end up granted on every site.
          callback.cont(PermissionRequestResult::ACCEPT);
          1
        }
        AppDecision::NoOpinion => {
          // The embedder prompts: nothing is granted until it answers.
          if let (Some(prompts), Some(browser), Some(callback)) =
            (&self.prompts, host.as_ref().and_then(|host| host.browser()), callback.as_deref())
          {
            let browser_id = browser.identifier();
            let id = remember(PendingRequest {
              browser_id,
              prompt_id: Some(prompt_id),
              answer: PendingAnswer::Prompt(callback.clone()),
            });
            prompts(
              browser_id,
              crate::BrowserStatus::PermissionRequest {
                id,
                origin: requesting_origin.map(ToString::to_string).unwrap_or_default(),
                kinds: kinds(requested_permissions, permission_kind),
              },
            );
            return 1;
          }

          // Chrome style displays the permission prompt UI.
          if !is_alloy_style(host.as_ref()) {
            return 0;
          }

          // Alloy style has no prompt UI, and its default handling is
          // `CEF_PERMISSION_RESULT_IGNORE`, which can leave the page's promise unresolved.
          // Accept instead, matching the behavior Alloy browsers had before permission
          // prompts were deferred to CEF.
          let Some(callback) = callback else {
            return 0;
          };

          // As above: accepting is what persists the grant.
          callback.cont(PermissionRequestResult::ACCEPT);
          1
        }
      }
    }

    fn on_dismiss_permission_prompt(
      &self,
      browser: Option<&mut Browser>,
      prompt_id: u64,
      _result: PermissionRequestResult,
    ) {
      let Some(browser_id) = browser.map(|browser| browser.identifier()) else {
        return;
      };
      // Also reached after an answer, when nothing is pending any more.
      // The removed request leaves the lock with its callback, which is
      // released only afterwards.
      let dismissed = with_pending(|pending| {
        let id = pending
          .iter()
          .find(|(_, request)| request.browser_id == browser_id && request.prompt_id == Some(prompt_id))
          .map(|(id, _)| *id)?;
        pending.remove(&id).map(|request| (id, request))
      });
      let dismissed = dismissed.map(|(id, request)| {
        drop(request);
        id
      });
      if let (Some(id), Some(prompts)) = (dismissed, &self.prompts) {
        prompts(browser_id, crate::BrowserStatus::PermissionDismissed { id });
      }
    }
  }
}

impl TauriCefPermissionHandler {
  /// Asks the application about every type set in `requested` and combines the
  /// answers, as [`AppDecision`] describes.
  ///
  /// `kind` names the [`PermissionKind`] of one request type; the two CEF request
  /// bitmasks number their types differently, so each entry point passes its own.
  fn decide(&self, requested: u32, kind: fn(u32) -> PermissionKind) -> AppDecision {
    let Some(handler) = &self.permission_request_handler else {
      return AppDecision::NoOpinion;
    };

    // A refusal short-circuits: nothing after it could weaken it, so the types past
    // it are not asked about. A handler that logs or keeps state therefore sees
    // only a prefix of a denied request. That is deliberate — the handler is a
    // policy predicate, not an event feed.
    let mut asked = false;
    let mut allowed_all = true;
    for permission in requested_permissions(requested) {
      asked = true;
      match handler(kind(permission)) {
        PermissionResponse::Deny => return AppDecision::Denied,
        PermissionResponse::Allow => {}
        PermissionResponse::Default => allowed_all = false,
      }
    }

    if asked && allowed_all {
      AppDecision::Allowed
    } else {
      AppDecision::NoOpinion
    }
  }
}

/// Whether answering `requested` through [`MediaAccessCallback::cont`] would hand
/// the page a desktop stream, which is never this handler's to give.
///
/// CEF builds the granted stream straight from the mask: a set
/// `DESKTOP_VIDEO_CAPTURE` bit with no requested device id synthesises a
/// `DesktopMediaID(TYPE_SCREEN, kFullDesktopScreenId)` and returns it, so
/// `getDisplayMedia()` resolves with the whole desktop and *no picker at all*. A
/// [`PermissionKind`] names no screen, window or tab, so an app writing
/// `.on_permission_request(|_| PermissionResponse::Allow)` would silently give any
/// page in the webview a full-desktop stream. Deferring to CEF keeps its picker in
/// front of the user, which is the only thing that can name what is shared.
///
/// The whole request is deferred, never a part of it: CEF requires
/// `allowed_permissions` to equal `required_permissions` for a request carrying the
/// device capture bits, so granting the device half of a mixed mask while
/// withholding the desktop half is not a legal answer.
///
/// A `Deny` is unaffected: `cont(0)` refuses every bit in the request, desktop
/// capture included.
fn grants_desktop_capture(requested: u32) -> bool {
  requested & DESKTOP_CAPTURE != 0
}

/// The individual permission types set in a CEF request bitmask.
fn requested_permissions(requested: u32) -> impl Iterator<Item = u32> {
  (0..u32::BITS)
    .map(move |bit| requested & (1 << bit))
    .filter(|permission| *permission != 0)
}

/// The [`PermissionKind`] of one `cef_permission_request_types_t` value.
fn permission_kind(permission: u32) -> PermissionKind {
  lookup_permission_kind(PERMISSION_KINDS, permission)
}

/// The [`PermissionKind`] of one `cef_media_access_permission_types_t` value.
fn media_permission_kind(permission: u32) -> PermissionKind {
  lookup_permission_kind(MEDIA_PERMISSION_KINDS, permission)
}

fn lookup_permission_kind(table: &[(u32, PermissionKind)], permission: u32) -> PermissionKind {
  table
    .iter()
    .find(|(candidate, _)| *candidate == permission)
    .map(|(_, kind)| *kind)
    .unwrap_or(PermissionKind::Other)
}

/// The content settings recording granted media capture.
///
/// Desktop capture has none: `getDisplayMedia` is gated by Chromium's source
/// picker rather than by a content setting, so there is nothing to record for it.
fn media_content_settings(granted: u32) -> Vec<ContentSettingTypes> {
  let mut settings = Vec::with_capacity(2);
  if granted & AUDIO_CAPTURE != 0 {
    settings.push(ContentSettingTypes::MEDIASTREAM_MIC);
  }
  if granted & VIDEO_CAPTURE != 0 {
    settings.push(ContentSettingTypes::MEDIASTREAM_CAMERA);
  }
  settings
}

/// The host of `browser`, when CEF hands one out.
fn browser_host(browser: Option<&mut Browser>) -> Option<BrowserHost> {
  browser.and_then(|browser| browser.host())
}

/// Whether `host` uses the Alloy runtime style, which provides no permission UI.
///
/// A browser whose host CEF did not hand out reports `false`, so that permission
/// handling is deferred to CEF, which is correct for the Chrome style Tauri
/// webviews use by default.
fn is_alloy_style(host: Option<&BrowserHost>) -> bool {
  host.is_some_and(|host| host.runtime_style() == RuntimeStyle::ALLOY)
}

/// Records granted media capture as content settings for `requesting_origin`.
///
/// `MediaAccessCallback::cont` grants the stream and nothing else: the grant is
/// invisible to Chromium's permission layer, so `navigator.permissions.query()`
/// keeps reporting `prompt` and `enumerateDevices()` keeps returning a redacted
/// list even though `getUserMedia` works. Writing the content setting is what
/// Chrome style does when the user accepts its prompt.
///
/// The permission *prompt* path must not use this: `cont(ACCEPT)` already persists
/// the grant, and the secondary pattern below is a wildcard — harmless for the two
/// media settings, which Chromium scopes to the requesting origin alone, but wrong
/// for anything it scopes to an (origin, top-level site) pair.
///
/// # The setting outlives the answer that wrote it
///
/// The media path is consulted on every `getUserMedia()` call, so a handler may
/// allow once and deny afterwards. Denying still refuses the stream, but the
/// content setting stays ALLOW, so `navigator.permissions.query()` keeps reporting
/// `granted` and `enumerateDevices()` keeps returning unredacted device labels for
/// that origin. An application that revokes camera or microphone access for good
/// should rewrite the setting itself; see the module docs.
///
/// Does nothing when the origin is unknown, because `set_content_setting` with no URL
/// changes the default for every origin rather than for this one, and nothing when CEF
/// handed out no host, because the request context is reached through it.
fn allow_content_settings(
  host: Option<&BrowserHost>,
  requesting_origin: Option<&CefString>,
  settings: &[ContentSettingTypes],
) {
  if requesting_origin.is_none() || settings.is_empty() {
    return;
  }

  let Some(context) = host.and_then(|host| host.request_context()) else {
    return;
  };

  for setting in settings {
    context.set_content_setting(
      requesting_origin,
      None,
      *setting,
      ContentSettingValues::ALLOW,
    );
  }
}
