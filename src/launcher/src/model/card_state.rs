//! What a game's card offers and the status shown for it (`cardState`, `statusFor`).

use super::facts::Facts;
use crate::constants::{menus as m, words_library as w};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardState {
    pub action: String,
    pub status: String,
    pub action_enabled: bool,
    pub needs_attention: bool,
    pub building: bool,
    pub running: bool,
}

fn state(action: &str, status: &str, enabled: bool, attention: bool) -> CardState {
    CardState {
        action: action.into(),
        status: status.into(),
        action_enabled: enabled,
        needs_attention: attention,
        ..CardState::default()
    }
}

/// `other_build`: some game is building; `tools_ready`: nxrs-build can build.
pub fn card_state(
    f: &Facts,
    running: bool,
    building: bool,
    other_build: bool,
    tools_ready: bool,
) -> CardState {
    if building {
        return CardState {
            building: true,
            ..state(m::CANCEL_BUILD, w::BUILDING, true, false)
        };
    }
    if running {
        return CardState {
            running: true,
            ..state(m::STOP, w::RUNNING, true, false)
        };
    }
    let can_build = !other_build && tools_ready && f.source_available;
    if f.has_build {
        if f.source_offline() {
            return state(m::PLAY, w::OFFLINE, false, true);
        }
        return state(m::PLAY, w::READY, true, false);
    }
    if f.update_pending {
        return state(m::REBUILD, w::UPDATE_CHANGED, can_build, true);
    }
    state(
        m::BUILD,
        if f.build_incomplete {
            w::INCOMPLETE
        } else {
            w::NEEDS_BUILD
        },
        can_build,
        true,
    )
}

/// The status the list and the status bar show.
pub fn status_for(f: &Facts, demo: bool, building: bool, running: bool) -> &'static str {
    if demo {
        w::PREVIEW
    } else if building {
        w::BUILDING
    } else if running {
        w::RUNNING
    } else if f.has_build && f.source_offline() {
        w::OFFLINE
    } else if f.update_pending {
        w::UPDATE_CHANGED_LIST
    } else if f.has_build {
        w::READY
    } else {
        w::BUILD_NEEDED
    }
}
