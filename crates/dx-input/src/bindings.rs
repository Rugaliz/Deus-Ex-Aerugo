//! Bindings (`key -> command line`) and aliases (`name -> command line`), read from the `[Engine.Input]` section of an
//! ini file. A command line holds commands separated by `|`; the first word of a command may be an alias.

use crate::key_code;
use std::collections::BTreeMap;

/// One command of a binding after alias expansion.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// `Button b`: the variable is 1 while the key is down.
    Button(String),
    /// `Pulse b`: the variable is 1 for the frame after the press.
    Pulse(String),
    /// `Toggle b`: the variable flips between 0 and 0x80 on each press.
    Toggle(String),
    /// `Axis a Speed=s`.
    Axis { name: String, speed: f32 },
    /// `OnRelease cmd`: the exec command runs when the key is released.
    OnRelease(String),
    /// Any other text: an exec function of the player with its arguments (`ActivateBelt 3`).
    Exec(String),
}

#[derive(Clone, Debug, Default)]
pub struct Bindings {
    keys: BTreeMap<u8, String>,
    aliases: Vec<(String, String)>,
}

impl Bindings {
    /// Reads the `[Engine.Input]` section of ini text: `Aliases[n]=(Command="...",Alias=Name)` lines and `Key=command` lines.
    /// Unknown key names are skipped; an empty command clears the key.
    pub fn from_ini(text: &str) -> Bindings {
        let mut b = Bindings::default();
        let mut inside = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                inside = line.eq_ignore_ascii_case("[Engine.Input]");
                continue;
            }
            if !inside || line.is_empty() || line.starts_with(';') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            if k.len() >= 7 && k[..7].eq_ignore_ascii_case("Aliases") {
                if let Some((alias, cmd)) = parse_alias(v) {
                    if !alias.is_empty() && !alias.eq_ignore_ascii_case("none") && !cmd.is_empty() {
                        b.aliases.retain(|(a, _)| !a.eq_ignore_ascii_case(&alias));
                        b.aliases.push((alias, cmd));
                    }
                }
            } else if let Some(code) = key_code(k) {
                if v.is_empty() {
                    b.keys.remove(&code);
                } else {
                    b.keys.insert(code, v.to_string());
                }
            }
        }
        b
    }

    /// The modern default layout of this port (see `DEFAULT_INI`).
    pub fn defaults() -> Bindings {
        Bindings::from_ini(DEFAULT_INI)
    }

    pub fn bind(&mut self, code: u8, command: &str) {
        if command.is_empty() {
            self.keys.remove(&code);
        } else {
            self.keys.insert(code, command.to_string());
        }
    }

    pub fn binding(&self, code: u8) -> Option<&str> {
        self.keys.get(&code).map(String::as_str)
    }

    pub fn alias(&self, name: &str) -> Option<&str> {
        self.aliases.iter().find(|(a, _)| a.eq_ignore_ascii_case(name)).map(|(_, c)| c.as_str())
    }

    /// Commands of a key's binding with aliases expanded (aliases are not expanded inside an alias, as in the original: `Jump` is an alias that runs the exec function `Jump`).
    pub fn commands(&self, code: u8) -> Vec<Command> {
        match self.keys.get(&code) {
            Some(line) => self.expand(line, 0),
            None => Vec::new(),
        }
    }

    fn expand(&self, line: &str, depth: u32) -> Vec<Command> {
        let mut out = Vec::new();
        for part in line.split('|') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let first = part.split_whitespace().next().unwrap_or("");
            if depth < 1 {
                if let Some(alias) = self.alias(first) {
                    out.extend(self.expand(alias, depth + 1));
                    continue;
                }
            }
            out.push(parse_command(part));
        }
        out
    }

    /// Writes the bindings back as an `[Engine.Input]` section.
    pub fn to_ini(&self) -> String {
        let mut s = String::from("[Engine.Input]\n");
        for (i, (a, c)) in self.aliases.iter().enumerate() {
            s += &format!("Aliases[{i}]=(Command=\"{c}\",Alias={a})\n");
        }
        for (code, cmd) in &self.keys {
            if let Some(n) = crate::key_name(*code) {
                s += &format!("{n}={cmd}\n");
            }
        }
        s
    }
}

fn parse_alias(v: &str) -> Option<(String, String)> {
    let c = v.find("Command=\"")? + 9;
    let end = v[c..].find('"')? + c;
    let command = v[c..end].trim().to_string();
    let a = v.to_ascii_lowercase().find("alias=")? + 6;
    let alias: String = v[a..].chars().take_while(|ch| ch.is_alphanumeric() || *ch == '_').collect();
    Some((alias, command))
}

fn parse_command(part: &str) -> Command {
    let mut words = part.split_whitespace();
    let head = words.next().unwrap_or("");
    let name = words.next().unwrap_or("").to_string();
    match head.to_ascii_lowercase().as_str() {
        "button" if !name.is_empty() => Command::Button(name),
        "pulse" if !name.is_empty() => Command::Pulse(name),
        "toggle" if !name.is_empty() => Command::Toggle(name),
        "axis" if !name.is_empty() => {
            let lower = part.to_ascii_lowercase();
            let speed = lower.find("speed=").and_then(|i| part[i + 6..].split_whitespace().next()).and_then(|t| t.parse::<f32>().ok()).unwrap_or(1.0);
            Command::Axis { name, speed }
        }
        "onrelease" => Command::OnRelease(part[head.len()..].trim().to_string()),
        _ => Command::Exec(part.to_string()),
    }
}

/// The default layout of this port, in the same format the game's `User.ini` uses (so it can be edited and written
/// back). Differences from the original layout, made to match what players expect from a PC game today:
/// WASD plus the arrows for movement, mouse look with the wheel cycling the belt, `Ctrl` (hold) and `C` (toggle) to
/// crouch, `Space` to jump, `F` and the right mouse button to interact (the game's "world" click), `R` to reload,
/// `Q`/`E` to lean, `Shift` held to walk quietly (the game walks slowly and runs by default), `I`/`Tab` inventory,
/// `G` goals, `H` health, `K` skills, `N` augmentations, `L` logs, `M` images, `J` conversations log, `P` pause,
/// `Escape` menu, the key left of `1` (`~` or `\` by layout) types a console line (`Talk`; the original binds `T`), `F3`..`F12` augmentations 1..10, `1`..`0` belt slots, `F5`/`F9` are the augmentations as in the game
/// (quick save and load are `NumPad+` and `NumPad/`, and `PageUp`/`PageDown`). A game controller is bound through the
/// `Joy*` keys: left stick moves, right stick looks, `A` jumps, `B` crouches, `X` reloads, `Y` interacts, the triggers
/// fire and use the item in hand, the bumpers cycle the belt, the d-pad picks inventory/goals/health/augmentations.
pub const DEFAULT_INI: &str = r#"[Engine.Input]
Aliases[0]=(Command="Button bFire | Fire",Alias=Fire)
Aliases[1]=(Command="Axis aBaseY Speed=+300.0",Alias=MoveForward)
Aliases[2]=(Command="Axis aBaseY Speed=-300.0",Alias=MoveBackward)
Aliases[3]=(Command="Axis aBaseX Speed=-150.0",Alias=TurnLeft)
Aliases[4]=(Command="Axis aBaseX Speed=+150.0",Alias=TurnRight)
Aliases[5]=(Command="Axis aStrafe Speed=-300.0",Alias=StrafeLeft)
Aliases[6]=(Command="Axis aStrafe Speed=+300.0",Alias=StrafeRight)
Aliases[7]=(Command="Jump | Axis aUp Speed=+300.0",Alias=Jump)
Aliases[8]=(Command="Button bDuck | Axis aUp Speed=-300.0",Alias=Duck)
Aliases[9]=(Command="Toggle bDuck",Alias=ToggleDuck)
Aliases[10]=(Command="Button bLook",Alias=Look)
Aliases[11]=(Command="Axis aLookUp Speed=+100.0",Alias=LookUp)
Aliases[12]=(Command="Axis aLookUp Speed=-100.0",Alias=LookDown)
Aliases[13]=(Command="Button bSnapLevel",Alias=CenterView)
Aliases[14]=(Command="Button bRun",Alias=Walking)
Aliases[15]=(Command="Button bStrafe",Alias=Strafe)
Aliases[16]=(Command="Axis aExtra0 Speed=+0.05",Alias=LeanRight)
Aliases[17]=(Command="Axis aExtra0 Speed=-0.05",Alias=LeanLeft)
LeftMouse=ParseLeftClick|Fire
RightMouse=ParseRightClick
MiddleMouse=PutInHand
MouseWheelUp=PrevBeltItem
MouseWheelDown=NextBeltItem
Tilde=Talk
Backslash=Talk
MouseX=Axis aMouseX Speed=6.0
MouseY=Axis aMouseY Speed=6.0
W=MoveForward
S=MoveBackward
A=StrafeLeft
D=StrafeRight
Up=MoveForward
Down=MoveBackward
Left=StrafeLeft
Right=StrafeRight
Space=Jump
LControl=Duck
C=ToggleDuck
LShift=Walking
CapsLock=ToggleWalk
Q=LeanLeft
E=LeanRight
F=ParseRightClick
R=ReloadWeapon
V=SwitchAmmo
Z=ToggleScope
X=ToggleLaser
G=ShowGoalsWindow
H=ShowHealthWindow
I=ShowInventoryWindow
Tab=ShowInventoryWindow
J=ShowConversationsWindow
K=ShowSkillsWindow
L=ShowLogsWindow
M=ShowImagesWindow
N=ShowAugmentationsWindow
B=DropItem
P=Pause
Pause=Pause
Escape=ShowMainMenu
PrintScrn=Shot
GreyPlus=QuickSave
GreySlash=QuickLoad
PageUp=QuickSave
PageDown=QuickLoad
1=ActivateBelt 1
2=ActivateBelt 2
3=ActivateBelt 3
4=ActivateBelt 4
5=ActivateBelt 5
6=ActivateBelt 6
7=ActivateBelt 7
8=ActivateBelt 8
9=ActivateBelt 9
0=ActivateBelt 0
F1=ShowInventoryWindow
F2=ShowGoalsWindow
F3=ActivateAugmentation 0
F4=ActivateAugmentation 1
F5=ActivateAugmentation 2
F6=ActivateAugmentation 3
F7=ActivateAugmentation 4
F8=ActivateAugmentation 5
F9=ActivateAugmentation 6
F10=ActivateAugmentation 7
F11=ActivateAugmentation 8
F12=ActivateAugmentation 9
LeftBracket=ToggleScope
RightBracket=ToggleLaser
Joy1=Jump
Joy2=ToggleDuck
Joy3=ReloadWeapon
Joy4=ParseRightClick
Joy5=PrevBeltItem
Joy6=NextBeltItem
Joy7=ShowGoalsWindow
Joy8=ShowMainMenu
Joy9=ToggleWalk
Joy10=ToggleScope
Joy11=ParseLeftClick|Fire
Joy12=ParseLeftClick
JoyU=
JoyPovUp=ShowInventoryWindow
JoyPovDown=ShowHealthWindow
JoyPovLeft=ShowGoalsWindow
JoyPovRight=ShowAugmentationsWindow
JoyX=Axis aStrafe Speed=+300.0
JoyY=Axis aBaseY Speed=-300.0
JoyZ=Axis aBaseX Speed=+150.0
JoyR=Axis aLookUp Speed=-100.0
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_expand_once() {
        let b = Bindings::from_ini(DEFAULT_INI);
        let w = key_code("W").unwrap();
        assert_eq!(b.commands(w), vec![Command::Axis { name: "aBaseY".into(), speed: 300.0 }]);
        let space = key_code("Space").unwrap();
        assert_eq!(b.commands(space), vec![Command::Exec("Jump".into()), Command::Axis { name: "aUp".into(), speed: 300.0 }]);
        // an alias may be named like the exec function it ends in (`Fire`, `Jump`): no endless expansion
        let left = key_code("LeftMouse").unwrap();
        assert_eq!(b.commands(left), vec![Command::Exec("ParseLeftClick".into()), Command::Button("bFire".into()), Command::Exec("Fire".into())]);
        assert_eq!(b.commands(key_code("3").unwrap()), vec![Command::Exec("ActivateBelt 3".into())]);
    }

    #[test]
    fn reads_the_games_own_user_ini_format() {
        let b = Bindings::from_ini("[Other]\nW=Nope\n[Engine.Input]\nAliases[1]=(Command=\"Axis aBaseY  Speed=+300.0\",Alias=MoveForward)\nUp=MoveForward\nhome=CenterView\nBogusKey=x\nA=\n");
        assert_eq!(b.commands(key_code("Up").unwrap()), vec![Command::Axis { name: "aBaseY".into(), speed: 300.0 }]);
        assert!(b.binding(key_code("W").unwrap()).is_none());
        assert_eq!(b.binding(key_code("Home").unwrap()), Some("CenterView"));
    }

    #[test]
    fn round_trips_through_ini_text() {
        let b = Bindings::defaults();
        let c = Bindings::from_ini(&b.to_ini());
        for code in 0..=255u8 {
            assert_eq!(b.binding(code), c.binding(code));
        }
        assert_eq!(b.alias("Duck"), c.alias("Duck"));
    }
}
