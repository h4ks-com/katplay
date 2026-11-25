//! The one controller the console sees, merged from the keyboard and the
//! game controllers.

use katui::proto::input::{Key, Mods};
use katui::rt::gamepad::Pad as Gamepad;
use katui::rt::gamepad::{PadAxis, PadButton};

use crate::chiaki::session::{FULL, Pad, button};

/// How far a stick has to move before it counts, so a worn one rests at zero.
const DEAD_ZONE: f32 = 0.12;

/// What one key stands in for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Button(u32),
    LeftTrigger,
    RightTrigger,
    LeftStick(i16, i16),
    RightStick(i16, i16),
}

impl Pad {
    /// Two controllers in one: any press is a press, the harder trigger wins,
    /// and the stick furthest from its centre wins, so a controller at rest
    /// never cancels one that moves.
    pub(crate) fn merge(self, other: Self) -> Self {
        let furthest = |mine: i16, theirs: i16| {
            if theirs.unsigned_abs() > mine.unsigned_abs() {
                theirs
            } else {
                mine
            }
        };
        Self {
            buttons: self.buttons | other.buttons,
            left_trigger: self.left_trigger.max(other.left_trigger),
            right_trigger: self.right_trigger.max(other.right_trigger),
            left: (
                furthest(self.left.0, other.left.0),
                furthest(self.left.1, other.left.1),
            ),
            right: (
                furthest(self.right.0, other.right.0),
                furthest(self.right.1, other.right.1),
            ),
        }
    }
}

/// The controller the held keys make: the left stick on WASD, the right
/// stick on the arrows, the direction pad on Shift and the arrows, the face
/// buttons laid out as on the pad on IJKL, the shoulders on E and U and the
/// triggers on R and O.
pub(crate) fn from_keys(held: impl IntoIterator<Item = (Key, Mods)>) -> Pad {
    held.into_iter()
        .filter_map(|(key, mods)| control(key, mods))
        .fold(Pad::default(), |pad, control| match control {
            Control::Button(flag) => Pad {
                buttons: pad.buttons | flag,
                ..pad
            },
            Control::LeftTrigger => Pad {
                left_trigger: u8::MAX,
                ..pad
            },
            Control::RightTrigger => Pad {
                right_trigger: u8::MAX,
                ..pad
            },
            Control::LeftStick(x, y) => Pad {
                left: (pad.left.0.saturating_add(x), pad.left.1.saturating_add(y)),
                ..pad
            },
            Control::RightStick(x, y) => Pad {
                right: (pad.right.0.saturating_add(x), pad.right.1.saturating_add(y)),
                ..pad
            },
        })
}

/// The controller the game controllers make together.
pub(crate) fn from_gamepads(gamepads: &[Gamepad]) -> Pad {
    gamepads
        .iter()
        .map(|gamepad| {
            const BUTTONS: [(PadButton, u32); 15] = [
                (PadButton::South, button::CROSS),
                (PadButton::East, button::CIRCLE),
                (PadButton::West, button::SQUARE),
                (PadButton::North, button::TRIANGLE),
                (PadButton::LeftBumper, button::L1),
                (PadButton::RightBumper, button::R1),
                (PadButton::Select, button::SHARE),
                (PadButton::Start, button::OPTIONS),
                (PadButton::LeftStick, button::L3),
                (PadButton::RightStick, button::R3),
                (PadButton::Up, button::UP),
                (PadButton::Down, button::DOWN),
                (PadButton::Left, button::LEFT),
                (PadButton::Right, button::RIGHT),
                (PadButton::Home, button::PS),
            ];
            Pad {
                buttons: home_chord(
                    BUTTONS
                        .iter()
                        .filter(|(name, _)| gamepad.pressed(*name))
                        .fold(0, |buttons, (_, flag)| buttons | flag),
                ),
                left_trigger: trigger(gamepad.button(PadButton::LeftTrigger)),
                right_trigger: trigger(gamepad.button(PadButton::RightTrigger)),
                left: (
                    stick(gamepad.axis(PadAxis::LeftX)),
                    stick(gamepad.axis(PadAxis::LeftY)),
                ),
                right: (
                    stick(gamepad.axis(PadAxis::RightX)),
                    stick(gamepad.axis(PadAxis::RightY)),
                ),
            }
        })
        .fold(Pad::default(), Pad::merge)
}

const KEYS: [(Key, Control); 22] = [
    (Key::Char('w'), Control::LeftStick(0, -FULL)),
    (Key::Char('a'), Control::LeftStick(-FULL, 0)),
    (Key::Char('s'), Control::LeftStick(0, FULL)),
    (Key::Char('d'), Control::LeftStick(FULL, 0)),
    (Key::Up, Control::RightStick(0, -FULL)),
    (Key::Left, Control::RightStick(-FULL, 0)),
    (Key::Down, Control::RightStick(0, FULL)),
    (Key::Right, Control::RightStick(FULL, 0)),
    (Key::Char('i'), Control::Button(button::TRIANGLE)),
    (Key::Char('l'), Control::Button(button::CIRCLE)),
    (Key::Char('k'), Control::Button(button::CROSS)),
    (Key::Char('j'), Control::Button(button::SQUARE)),
    (Key::Char('e'), Control::Button(button::L1)),
    (Key::Char('u'), Control::Button(button::R1)),
    (Key::Char('r'), Control::LeftTrigger),
    (Key::Char('o'), Control::RightTrigger),
    (Key::Char('c'), Control::Button(button::L3)),
    (Key::Char('n'), Control::Button(button::R3)),
    (Key::Char('v'), Control::Button(button::SHARE)),
    (Key::Char('b'), Control::Button(button::OPTIONS)),
    (Key::Char('p'), Control::Button(button::PS)),
    (Key::Char('m'), Control::Button(button::TOUCHPAD)),
];

const SHIFTED_KEYS: [(Key, Control); 4] = [
    (Key::Left, Control::Button(button::LEFT)),
    (Key::Right, Control::Button(button::RIGHT)),
    (Key::Up, Control::Button(button::UP)),
    (Key::Down, Control::Button(button::DOWN)),
];

/// The operating system keeps the Home button of many controllers for
/// itself, so Share and Options held together stand in for PS.
fn home_chord(buttons: u32) -> u32 {
    let chord = button::SHARE | button::OPTIONS;
    if buttons & chord == chord {
        buttons & !chord | button::PS
    } else {
        buttons
    }
}

fn control(key: Key, mods: Mods) -> Option<Control> {
    if mods.ctrl || mods.alt {
        return None;
    }
    let table: &[(Key, Control)] = if mods.shift { &SHIFTED_KEYS } else { &KEYS };
    table
        .iter()
        .find_map(|(bound, control)| (*bound == key).then_some(*control))
}

#[allow(clippy::cast_sign_loss, reason = "the value is clamped to 0..=1 first")]
fn trigger(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * f32::from(u8::MAX)).round() as u8
}

fn stick(value: f32) -> i16 {
    let value = if value.abs() < DEAD_ZONE {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    };
    (value * f32::from(FULL)).round() as i16
}

#[cfg(test)]
mod tests {
    use katui::proto::input::{Key, Mods};
    use katui::rt::gamepad::Pad as Gamepad;
    use katui::rt::gamepad::PadButton;

    use super::{from_gamepads, from_keys};
    use crate::chiaki::session::{FULL, Pad, button};

    const NONE: Mods = Mods::NONE;
    const SHIFT: Mods = Mods {
        shift: true,
        ..Mods::NONE
    };

    fn gamepad(pressed: PadButton, left_y: f32, right_trigger: f32) -> Gamepad {
        let mut buttons = [0.0; 17];
        buttons[pressed as usize] = 1.0;
        buttons[PadButton::RightTrigger as usize] = right_trigger;
        Gamepad {
            index: 0,
            name: "DualSense".to_owned(),
            buttons,
            axes: [0.0, left_y, 0.0, 0.0],
        }
    }

    #[test]
    fn keys_make_the_controller_they_stand_in_for() {
        let keys = [
            (Key::Char('w'), NONE),
            (Key::Char('d'), NONE),
            (Key::Down, NONE),
            (Key::Up, SHIFT),
            (Key::Char('k'), NONE),
            (Key::Char('r'), NONE),
            (Key::Char('q'), NONE),
            (Key::Char('k'), SHIFT),
            (
                Key::Char('c'),
                Mods {
                    ctrl: true,
                    ..Mods::NONE
                },
            ),
        ];
        assert_eq!(
            from_keys(keys),
            Pad {
                buttons: button::CROSS | button::UP,
                left_trigger: u8::MAX,
                right_trigger: 0,
                left: (FULL, -FULL),
                right: (0, FULL),
            }
        );
    }

    #[test]
    fn a_merge_keeps_every_press_and_the_stick_that_moves() {
        let resting = Pad::default();
        let moving = Pad {
            left: (100, -200),
            buttons: button::CROSS,
            left_trigger: 10,
            ..Pad::default()
        };
        let other = Pad {
            left: (5000, 0),
            buttons: button::CIRCLE,
            left_trigger: 200,
            right_trigger: 3,
            ..Pad::default()
        };
        assert_eq!(
            resting.merge(moving),
            moving,
            "a controller at rest adds nothing"
        );
        assert_eq!(moving.merge(other).buttons, button::CROSS | button::CIRCLE);
        assert_eq!(
            moving.merge(other).left,
            (5000, -200),
            "the furthest axis of each wins"
        );
        assert_eq!(
            (
                moving.merge(other).left_trigger,
                moving.merge(other).right_trigger
            ),
            (200, 3)
        );
    }

    #[test]
    fn a_game_controller_maps_by_the_standard_layout() {
        let pad = from_gamepads(&[
            gamepad(PadButton::South, -1.0, 0.5),
            gamepad(PadButton::North, 0.05, 0.0),
        ]);
        assert_eq!(pad.buttons, button::CROSS | button::TRIANGLE);
        assert_eq!(
            pad.left,
            (0, -FULL),
            "the second pad sits inside the dead zone"
        );
        assert_eq!(pad.right_trigger, 128);
        assert_eq!(from_gamepads(&[]), Pad::default());
    }

    #[test]
    fn share_and_options_together_press_ps() {
        let mut both = gamepad(PadButton::Select, 0.0, 0.0);
        both.buttons[PadButton::Start as usize] = 1.0;
        assert_eq!(from_gamepads(&[both]).buttons, button::PS);
        assert_eq!(
            from_gamepads(&[gamepad(PadButton::Select, 0.0, 0.0)]).buttons,
            button::SHARE
        );
    }
}
