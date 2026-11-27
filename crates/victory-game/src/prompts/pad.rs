//! The gamepad controls the title's prompts picture, and where the key of
//! each comes from.
//!
//! The keys are not kept here: the keyboard layer (`[controls]`, in
//! victory-hle) owns the bindings. The front end hands this module a
//! function that answers, for a pad output name (`A`, `LB`, `DpadUp`,
//! `LeftUp`, ...), the virtual key the layout in force binds to it.
//!
//! The control list is imjustprism's (Marvisak/VIctory#3433).

use std::sync::OnceLock;

/// Windows virtual keys the labels know. Mouse buttons are virtual keys too.
pub const VK_LBUTTON: u8 = 0x01;
pub const VK_RBUTTON: u8 = 0x02;
pub const VK_MBUTTON: u8 = 0x04;
pub const VK_XBUTTON1: u8 = 0x05;
pub const VK_XBUTTON2: u8 = 0x06;
pub const VK_BACK: u8 = 0x08;
pub const VK_TAB: u8 = 0x09;
pub const VK_RETURN: u8 = 0x0D;
pub const VK_CAPITAL: u8 = 0x14;
pub const VK_ESCAPE: u8 = 0x1B;
pub const VK_SPACE: u8 = 0x20;
pub const VK_LEFT: u8 = 0x25;
pub const VK_UP: u8 = 0x26;
pub const VK_RIGHT: u8 = 0x27;
pub const VK_DOWN: u8 = 0x28;
pub const VK_LSHIFT: u8 = 0xA0;
pub const VK_RSHIFT: u8 = 0xA1;
pub const VK_LCONTROL: u8 = 0xA2;
pub const VK_RCONTROL: u8 = 0xA3;
pub const VK_LMENU: u8 = 0xA4;
pub const VK_RMENU: u8 = 0xA5;

/// Declares the gamepad's controls, the list of all of them and each one's
/// pad output name (the names of the keyboard layer's bindings) from one
/// list, so that none of the three can miss a control.
macro_rules! pad_controls {
    ($($control:ident => $output:literal),* $(,)?) => {
        /// A control of a gamepad.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum PadControl {
            $($control),*
        }

        impl PadControl {
            /// Every control, in the order declared.
            pub const ALL: &[Self] = &[$(Self::$control),*];

            /// The name the keyboard layer's bindings give this control.
            #[must_use]
            pub const fn output_name(self) -> &'static str {
                match self {
                    $(Self::$control => $output),*
                }
            }
        }
    };
}

pad_controls!(
    Menu => "Menu",
    View => "View",
    A => "A",
    B => "B",
    X => "X",
    Y => "Y",
    Up => "DpadUp",
    Down => "DpadDown",
    Left => "DpadLeft",
    Right => "DpadRight",
    LeftShoulder => "LB",
    RightShoulder => "RB",
    LeftStick => "LS",
    RightStick => "RS",
    LeftTrigger => "LT",
    RightTrigger => "RT",
    LeftStickUp => "LeftUp",
    LeftStickDown => "LeftDown",
    LeftStickLeft => "LeftLeft",
    LeftStickRight => "LeftRight",
    RightStickUp => "RightUp",
    RightStickDown => "RightDown",
    RightStickLeft => "RightLeft",
    RightStickRight => "RightRight",
);

/// The key the bindings in force give a pad output name; `None` when none.
pub type KeySource = fn(&'static str) -> Option<u8>;

static KEY_SOURCE: OnceLock<KeySource> = OnceLock::new();

// Tests run on threads of their own and each sets the source it needs.
#[cfg(test)]
thread_local! {
    static TEST_SOURCE: std::cell::Cell<Option<KeySource>> = const { std::cell::Cell::new(None) };
}

/// Sets where keys come from, once, before the hook is placed. `false` when
/// a source was set before.
#[cfg(not(test))]
pub fn set_key_source(source: KeySource) -> bool {
    KEY_SOURCE.set(source).is_ok()
}

/// Sets where keys come from, for the calling thread only.
#[cfg(test)]
pub fn set_key_source(source: KeySource) -> bool {
    TEST_SOURCE.with(|cell| cell.set(Some(source)));
    true
}

/// The key the bindings in force give `control`, now; `None` when no source
/// is set or nothing is bound to it.
pub(crate) fn bound_key(control: PadControl) -> Option<u8> {
    #[cfg(test)]
    let source = TEST_SOURCE.with(std::cell::Cell::get);
    #[cfg(not(test))]
    let source = KEY_SOURCE.get().copied();
    source.and_then(|source| source(control.output_name()))
}

/// Keys from the classic stand-in table, by output name, for tests.
#[cfg(test)]
pub(crate) fn use_classic_keys() {
    fn classic(name: &'static str) -> Option<u8> {
        PadControl::ALL
            .iter()
            .find(|control| control.output_name() == name)
            .map(|&control| classic_key(control))
    }
    set_key_source(classic);
}

/// The classic stand-in keys, for tests that need a key for every control.
#[cfg(test)]
pub(crate) fn classic_key(control: PadControl) -> u8 {
    use PadControl as P;
    match control {
        P::Menu => VK_ESCAPE,
        P::View => VK_TAB,
        P::A => VK_RETURN,
        P::B => VK_BACK,
        P::X => b'X',
        P::Y => b'Y',
        P::Up => VK_UP,
        P::Down => VK_DOWN,
        P::Left => VK_LEFT,
        P::Right => VK_RIGHT,
        P::LeftShoulder => b'Q',
        P::RightShoulder => b'E',
        P::LeftStick => b'F',
        P::RightStick => b'H',
        P::LeftTrigger => b'Z',
        P::RightTrigger => b'C',
        P::LeftStickUp => b'W',
        P::LeftStickDown => b'S',
        P::LeftStickLeft => b'A',
        P::LeftStickRight => b'D',
        P::RightStickUp => b'I',
        P::RightStickDown => b'K',
        P::RightStickLeft => b'J',
        P::RightStickRight => b'L',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_has_an_output_name_of_its_own() {
        let mut names: Vec<&str> = PadControl::ALL.iter().map(|c| c.output_name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PadControl::ALL.len());
    }

    #[test]
    fn every_classic_control_has_a_key_of_its_own() {
        let mut keys: Vec<u8> = PadControl::ALL.iter().map(|&c| classic_key(c)).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), PadControl::ALL.len());
    }
}
