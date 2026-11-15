//! Screen resolution and fullscreen: what the game's Display menu lists and switches (its scripts ask the engine with
//! the console commands `GetRes`, `GetCurrentRes` and `SetRes`), the choice remembered in the port's `DeusEx.ini`.
//!
//! The window is fullscreen at the display's own resolution (or a plain window); the resolution setting is the size the
//! game renders at, scaled to the window keeping its aspect ratio (black bars when the aspects differ). Any resolution
//! from 4:3 to 32:9 works on any display, without changing the display's video mode.

use crate::Session;

/// The `DeusEx.ini` section and keys the original's window client uses.
const SECTION: &str = "WinDrv.WindowsClient";

/// Common resolutions offered besides the display's own modes, 4:3 to 32:9.
const COMMON: [(u32, u32); 22] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 960),
    (1600, 1200),
    (1280, 800),
    (1680, 1050),
    (1920, 1200),
    (2560, 1600),
    (1280, 720),
    (1366, 768),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
    (2560, 1080),
    (3440, 1440),
    (3840, 1600),
    (5120, 2160),
    (3840, 1080),
    (5120, 1440),
    (7680, 2160),
];

/// The game's menu reads at most this many entries of `GetRes`.
pub const MENU_SLOTS: usize = 16;

/// Aspect ratios offered: 4:3 to 32:9 (with a little slack for modes like 1366x768 or 1280x1024's 5:4 left out).
fn aspect_ok(w: u32, h: u32) -> bool {
    let a = w as f32 / h as f32;
    (4.0 / 3.0 - 0.01..=32.0 / 9.0 + 0.01).contains(&a)
}

/// The resolutions to offer for a display of `native` size with these video modes: common ones and the display's own,
/// 4:3 to 32:9, at least 640x480, no larger than the display in either direction, always including the display's own
/// size; at most `MENU_SLOTS`, thinning out the aspect ratio with the most entries (smallest first) when there are more.
/// Sorted by width, then height.
pub fn resolution_list(native: (u32, u32), display_modes: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let fits = |&(w, h): &(u32, u32)| w >= 640 && h >= 480 && w <= native.0 && h <= native.1 && aspect_ok(w, h);
    let mut list: Vec<(u32, u32)> = COMMON.iter().chain(display_modes).copied().filter(fits).collect();
    list.push(native);
    list.sort();
    list.dedup();
    // aspect class of a resolution, to two decimals (16:9 and 1366x768 count as the same)
    let class = |&(w, h): &(u32, u32)| (w as f32 / h as f32 * 100.0 / 2.0).round() as i32;
    while list.len() > MENU_SLOTS {
        let mut counts: std::collections::HashMap<i32, usize> = Default::default();
        for r in list.iter().filter(|r| **r != native) {
            *counts.entry(class(r)).or_default() += 1;
        }
        let Some((&busiest, _)) = counts.iter().max_by_key(|(c, n)| (**n, -**c)) else { break };
        // the smallest entry of that class goes
        let Some(i) = list.iter().position(|r| *r != native && class(r) == busiest) else { break };
        list.remove(i);
    }
    list
}

/// Parses `WxH` (also `WxHxDepth` and a trailing `w`/`f` for windowed/fullscreen, as the original accepts).
pub fn parse_res(s: &str) -> Option<(u32, u32, Option<bool>)> {
    let s = s.trim();
    let (s, fullscreen) = match s.chars().last()? {
        'w' | 'W' => (&s[..s.len() - 1], Some(false)),
        'f' | 'F' => (&s[..s.len() - 1], Some(true)),
        _ => (s, None),
    };
    let mut parts = s.split(['x', 'X']);
    let w: u32 = parts.next()?.trim().parse().ok()?;
    let h: u32 = parts.next()?.trim().parse().ok()?;
    (w >= 320 && h >= 200 && w <= 16384 && h <= 16384).then_some((w, h, fullscreen))
}

/// The display state shared with the console commands; the window loop applies `pending`.
#[derive(Debug, Clone, Default)]
pub struct Display {
    /// The display's own resolution (desktop mode).
    pub native: (u32, u32),
    /// What `GetRes` lists.
    pub modes: Vec<(u32, u32)>,
    pub fullscreen: bool,
    /// The current resolution: the render size in fullscreen, the window size in a window.
    pub resolution: (u32, u32),
    /// A change asked for by `SetRes`: resolution and, if given, fullscreen.
    pub pending: Option<((u32, u32), Option<bool>)>,
    /// The resolution each mode had last (fullscreen, window), for switching between them.
    pub last_fullscreen: Option<(u32, u32)>,
    pub last_window: Option<(u32, u32)>,
}

impl Display {
    /// Answers `GetRes`, `GetCurrentRes` and `SetRes`; `None` for other commands.
    pub fn console(&mut self, cmd: &str) -> Option<String> {
        let mut words = cmd.split_whitespace();
        let verb = words.next()?.to_ascii_lowercase();
        match verb.as_str() {
            "getres" => Some(self.modes.iter().map(|(w, h)| format!("{w}x{h}")).collect::<Vec<_>>().join(" ")),
            "getcurrentres" => Some(format!("{}x{}", self.resolution.0, self.resolution.1)),
            "setres" => {
                let (w, h, fs) = parse_res(words.next()?)?;
                self.pending = Some(((w, h), fs));
                Some(String::new())
            }
            "togglefullscreen" => {
                self.toggle_fullscreen();
                Some(String::new())
            }
            _ => None,
        }
    }
}

impl Display {
    /// Asks for the other mode at the resolution it had last (fullscreen: the display's own size at first; a window:
    /// 1600x1200 at first).
    pub fn toggle_fullscreen(&mut self) {
        let to_full = !self.fullscreen;
        let res = if to_full {
            self.last_fullscreen.unwrap_or(self.native)
        } else {
            self.last_window.unwrap_or((1600, 1200))
        };
        self.pending = Some((res, Some(to_full)));
    }
}

impl Session {
    /// The saved display settings (only what this port saved: the shipped ini's 640x480 is ignored): fullscreen
    /// resolution, window size, start fullscreen.
    pub fn saved_display(&mut self) -> (Option<(u32, u32)>, Option<(u32, u32)>, Option<bool>) {
        let Some(c) = self.vm.reg.config.as_mut() else { return (None, None, None) };
        let mut num = |k: &str| c.user_value("System", SECTION, k).and_then(|v| v.trim().parse::<u32>().ok());
        let full = num("FullscreenViewportX").zip(num("FullscreenViewportY"));
        let win = num("WindowedViewportX").zip(num("WindowedViewportY"));
        let start = c.user_value("System", SECTION, "StartupFullscreen").map(|v| v.trim().eq_ignore_ascii_case("true"));
        (full, win, start)
    }

    /// Remembers the display settings in the port's `DeusEx.ini`.
    pub fn save_display(&mut self, d: &Display) {
        let Some(c) = self.vm.reg.config.as_mut() else { return };
        let (x, y) = if d.fullscreen { ("FullscreenViewportX", "FullscreenViewportY") } else { ("WindowedViewportX", "WindowedViewportY") };
        c.set("System", SECTION, x, &d.resolution.0.to_string());
        c.set("System", SECTION, y, &d.resolution.1.to_string());
        c.set("System", SECTION, "StartupFullscreen", if d.fullscreen { "True" } else { "False" });
        let _ = c.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_1080p_display_gets_up_to_16_resolutions_including_its_own() {
        let l = resolution_list((1920, 1080), &[(1920, 1080), (1680, 1050), (1280, 1024), (1024, 768), (800, 600)]);
        assert!(l.len() <= MENU_SLOTS);
        assert!(l.contains(&(1920, 1080)));
        assert!(l.contains(&(640, 480)) && l.contains(&(1024, 768)));
        assert!(!l.contains(&(1280, 1024)), "5:4 is outside 4:3..32:9");
        assert!(l.iter().all(|&(w, h)| w <= 1920 && h <= 1080));
        let mut sorted = l.clone();
        sorted.sort();
        assert_eq!(l, sorted);
    }

    #[test]
    fn a_super_ultrawide_display_offers_32_9_and_16_9() {
        let l = resolution_list((5120, 1440), &[(5120, 1440)]);
        assert!(l.len() <= MENU_SLOTS);
        assert!(l.contains(&(5120, 1440)) && l.contains(&(3840, 1080)) && l.contains(&(2560, 1440)) && l.contains(&(1920, 1080)));
        assert!(l.contains(&(3440, 1440)) && l.contains(&(1024, 768)));
    }

    #[test]
    fn a_4k_display_keeps_every_aspect_within_16_entries() {
        let l = resolution_list((3840, 2160), &[]);
        assert_eq!(l.len(), MENU_SLOTS);
        for want in [(3840, 2160), (1600, 1200), (2560, 1600), (3440, 1440), (3840, 1080), (5120, 1440)] {
            if want.0 <= 3840 {
                assert!(l.contains(&want), "{want:?} missing from {l:?}");
            }
        }
    }

    #[test]
    fn setres_parses_like_the_original() {
        assert_eq!(parse_res("1920x1080"), Some((1920, 1080, None)));
        assert_eq!(parse_res("800x600x32"), Some((800, 600, None)));
        assert_eq!(parse_res("1280x720w"), Some((1280, 720, Some(false))));
        assert_eq!(parse_res("nonsense"), None);
        let mut d = Display { modes: vec![(800, 600), (1920, 1080)], resolution: (1920, 1080), ..Default::default() };
        assert_eq!(d.console("GetRes").as_deref(), Some("800x600 1920x1080"));
        assert_eq!(d.console("GetCurrentRes").as_deref(), Some("1920x1080"));
        d.console("SetRes 800x600");
        assert_eq!(d.pending, Some(((800, 600), None)));
        d.native = (1920, 1080);
        d.fullscreen = true;
        d.last_window = Some((1280, 720));
        d.console("TOGGLEFULLSCREEN");
        assert_eq!(d.pending, Some(((1280, 720), Some(false))));
        assert_eq!(d.console("Something else"), None);
    }
}
