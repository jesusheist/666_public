//! The pad pressed from the debug MCP server (`victory_mcp::remote_pad`),
//! as the title's pad state. `gameinput` ORs it into host pad 0 on every
//! poll, so it works without window focus and without a physical pad.

use super::gameinput::PadState;

/// What the remote pad shows now; `None` until an MCP pad tool was used.
pub(super) fn read() -> Option<PadState> {
    victory_mcp::remote_pad::read().map(convert)
}

fn convert(s: victory_mcp::remote_pad::Snapshot) -> PadState {
    PadState {
        buttons: s.buttons,
        left_trigger: s.left_trigger,
        right_trigger: s.right_trigger,
        left_x: s.left_x,
        left_y: s.left_y,
        right_x: s.right_x,
        right_y: s.right_y,
    }
}

#[cfg(test)]
mod tests {
    use victory_mcp::remote_pad::bit;

    use super::super::gameinput::{
        A, B, DOWN, LEFT, LEFT_SHOULDER, LEFT_STICK, MENU, RIGHT, RIGHT_SHOULDER, RIGHT_STICK,
        UP, VIEW, X, Y,
    };
    use super::*;

    #[test]
    fn button_bits_match_the_title_pad() {
        let pairs = [
            (bit::MENU, MENU),
            (bit::VIEW, VIEW),
            (bit::A, A),
            (bit::B, B),
            (bit::X, X),
            (bit::Y, Y),
            (bit::UP, UP),
            (bit::DOWN, DOWN),
            (bit::LEFT, LEFT),
            (bit::RIGHT, RIGHT),
            (bit::LEFT_SHOULDER, LEFT_SHOULDER),
            (bit::RIGHT_SHOULDER, RIGHT_SHOULDER),
            (bit::LEFT_STICK, LEFT_STICK),
            (bit::RIGHT_STICK, RIGHT_STICK),
        ];
        for (remote, title) in pairs {
            assert_eq!(remote, title);
        }
    }

    #[test]
    fn a_snapshot_converts_field_for_field() {
        let pad = convert(victory_mcp::remote_pad::Snapshot {
            buttons: A | UP,
            left_trigger: 1.0,
            right_trigger: 0.0,
            left_x: -1.0,
            left_y: 0.5,
            right_x: 0.25,
            right_y: -0.75,
        });
        assert_eq!(pad.buttons, A | UP);
        assert_eq!((pad.left_trigger, pad.right_trigger), (1.0, 0.0));
        assert_eq!((pad.left_x, pad.left_y, pad.right_x, pad.right_y), (-1.0, 0.5, 0.25, -0.75));
    }
}
