// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use cef::*;

#[cfg(target_os = "linux")]
type CefOsEvent<'a> = Option<&'a mut cef::sys::XEvent>;
#[cfg(target_os = "macos")]
type CefOsEvent<'a> = *mut u8;
#[cfg(windows)]
type CefOsEvent<'a> = Option<&'a mut cef::sys::MSG>;

wrap_keyboard_handler! {
  pub struct TauriCefKeyboardHandler {
    devtools_enabled: bool,
    shortcut_filter: Option<std::sync::Arc<crate::ShortcutFilter>>,
    status: Option<std::sync::Arc<crate::browser_status::ScopedStatus>>,
  }

  impl KeyboardHandler {
    fn on_pre_key_event(
      &self,
      _browser: Option<&mut Browser>,
      event: Option<&KeyEvent>,
      _os_event: CefOsEvent<'_>,
      _is_keyboard_shortcut: Option<&mut ::std::os::raw::c_int>,
    ) -> ::std::os::raw::c_int {
      // Embedder shortcuts are claimed before the page: only a raw key-down
      // with a shortcut modifier is offered, so typing and IME never are.
      if let (Some(filter), Some(status), Some(browser), Some(event)) =
        (&self.shortcut_filter, &self.status, _browser.as_deref(), event)
        && let Some(chord) = chord(event)
        && filter(chord)
      {
        status(browser.identifier(), crate::BrowserStatus::Shortcut(chord));
        return 1;
      }
      // If devtools is disabled, block devtools keyboard shortcuts.
      if !self.devtools_enabled {
        let Some(event) = event else {
          return 0;
        };

        // Check if this is a keydown event.
        use cef::sys::cef_key_event_type_t;
        let keydown_type: cef::KeyEventType = cef_key_event_type_t::KEYEVENT_RAWKEYDOWN.into();
        if event.type_ != keydown_type {
          return 0;
        }

        // Get modifier keys.
        use cef::sys::cef_event_flags_t;
        #[cfg(windows)]
        let modifiers = event.modifiers as i32;
        #[cfg(not(windows))]
        let modifiers = event.modifiers;

        #[cfg(not(target_os = "macos"))]
        let ctrl = (modifiers & (cef_event_flags_t::EVENTFLAG_CONTROL_DOWN.0)) != 0;
        #[cfg(not(target_os = "macos"))]
        let shift = (modifiers & (cef_event_flags_t::EVENTFLAG_SHIFT_DOWN.0)) != 0;

        let key_code = event.windows_key_code;

        // Block F12 (key code 123).
        if key_code == 123 {
          if let Some(is_keyboard_shortcut) = _is_keyboard_shortcut {
            *is_keyboard_shortcut = 1;
          }
          return 1;
        }

        // Block Ctrl+Shift+I (key code 73 = 'I') on Linux/Windows.
        #[cfg(not(target_os = "macos"))]
        if key_code == 73 && ctrl && shift {
          if let Some(is_keyboard_shortcut) = _is_keyboard_shortcut {
            *is_keyboard_shortcut = 1;
          }
          return 1;
        }

        // Block Cmd+Opt+I on macOS.
        #[cfg(target_os = "macos")]
        {
          let meta = (modifiers & cef_event_flags_t::EVENTFLAG_COMMAND_DOWN.0) != 0;
          let alt = (modifiers & cef_event_flags_t::EVENTFLAG_ALT_DOWN.0) != 0;
          if key_code == 73 && meta && alt {
            if let Some(is_keyboard_shortcut) = _is_keyboard_shortcut {
              *is_keyboard_shortcut = 1;
            }
            return 1;
          }
        }
      }

      0
    }
  }
}

/// The chord of a raw key-down that carries a shortcut modifier.
fn chord(event: &KeyEvent) -> Option<crate::KeyChord> {
  use cef::sys::{cef_event_flags_t, cef_key_event_type_t};
  let key_down: cef::KeyEventType = cef_key_event_type_t::KEYEVENT_RAWKEYDOWN.into();
  if event.type_ != key_down {
    return None;
  }
  #[cfg(windows)]
  let modifiers = event.modifiers as i32;
  #[cfg(not(windows))]
  let modifiers = event.modifiers;
  let has = |flag: cef_event_flags_t| (modifiers & flag.0) != 0;
  let chord = crate::KeyChord {
    key_code: event.windows_key_code,
    command: has(cef_event_flags_t::EVENTFLAG_COMMAND_DOWN),
    control: has(cef_event_flags_t::EVENTFLAG_CONTROL_DOWN),
    shift: has(cef_event_flags_t::EVENTFLAG_SHIFT_DOWN),
    alt: has(cef_event_flags_t::EVENTFLAG_ALT_DOWN),
  };
  (chord.command || chord.control).then_some(chord)
}
