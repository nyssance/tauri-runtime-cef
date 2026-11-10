// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::sync::Arc;

use cef::*;

wrap_find_handler! {
  pub struct TauriCefFindHandler {
    status: Arc<crate::browser_status::ScopedStatus>,
  }

  impl FindHandler {
    fn on_find_result(
      &self,
      browser: Option<&mut Browser>,
      _identifier: ::std::os::raw::c_int,
      count: ::std::os::raw::c_int,
      _selection_rect: Option<&Rect>,
      active_match_ordinal: ::std::os::raw::c_int,
      final_update: ::std::os::raw::c_int,
    ) {
      if let Some(browser) = browser {
        (self.status)(
          browser.identifier(),
          crate::BrowserStatus::Find {
            // A negative number leaves the previous one standing.
            count: u32::try_from(count).ok(),
            active: u32::try_from(active_match_ordinal).ok(),
            final_update: final_update != 0,
          },
        );
      }
    }
  }
}
