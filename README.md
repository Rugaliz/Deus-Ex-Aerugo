# Deus Ex Aerugo

A new engine for **Deus Ex** (2000) that lets you play the original game on today's Windows and Linux PCs. You keep
your own copy of the game; Aerugo only replaces the old program that runs it. Missions, characters, story and
gameplay are exactly the original's, because Aerugo runs the game's own scripts and data unchanged.

## What you get

- **Runs on modern systems.** A native Windows 10/11 and Linux program. No compatibility patches, no Wine, nothing
  else to install: one small file.
- **Any screen.** Your monitor's own resolution, including widescreen and ultrawide (16:9, 21:9, 32:9), with a wider
  view on wider screens instead of a stretched picture.
- **Smooth and fast.** Hundreds of frames per second on a modern PC (well over a thousand on a recent one), ready for
  high refresh rate monitors. The picture is drawn on a thread of its own while the game runs the next moment, which
  keeps the frame rate steady on slower PCs and laptops too. Choose how vertical sync works (off, on, adaptive for
  FreeSync/G-Sync, fast) and set a frame rate limit (*Auto*: your monitor's refresh rate).
- **A sharper, smoother picture.** Every texture and effect at full quality, plus modern options in *Display →
  Advanced Graphics*: anisotropic texture filtering, anti-aliasing (MSAA, with smooth grates and fences), 16-bit colour
  (no banding in fog and dark areas), smooth lightmap shadows and per-pixel lighting on characters and objects; on
  graphics cards with ray tracing, ray-traced shadows of characters, objects, trees and doors and ray-traced ambient
  occlusion. A field-of-view setting, and how many blood stains, bullet holes and scorch marks stay. Each option can
  be set back to the original look.
- **Quicker conversations.** Pick a dialogue choice with the number keys `1` to `9`, as well as with the mouse.
- **Precise mouse aiming.** Tell the game your mouse's DPI, and the sensitivity slider shows how far you move the
  mouse for a full turn, so aiming feels the same on any mouse.
- **Better sound.** More sound channels than the original's 16, so sounds no longer cut each other off, at your
  system's own audio quality, and a loudness control (*Sound* settings) that keeps rooms full of machines from getting
  too loud; *Original* mixes as the original game. A dialogue line cut short (skipped, or ended by the next) fades out
  instead of clicking. The original's sound and music engine is recreated faithfully.
- **No holes in the levels.** Some maps shipped with broken culling data that made the original skip whole rooms and
  doors from most angles (the Hong Kong Helibase's corridor door and stairs room), and some underwater walls without a
  surface, so you saw through the rock to the far end of the canal (Hong Kong's Wan Chai canal); Aerugo draws them.
- **Leaves your game alone.** Settings and saves go to your user folder, never into the game folder, so the original
  game keeps working side by side.
- **Easy mods.** Drop texture packs, models and other mods into a `Mods` folder as zip files; no installers, no
  overwritten game files. A mod kit to view, export and re-import the game's models and textures comes with it.

> **Status: the engine is complete, and the game has been played up to Vandenberg** (the intro, the training and
> every mission from Liberty Island to Vandenberg). The last missions (the ocean lab, the missile silo and Area 51)
> and the endings load and run but have not been played from start to finish, so expect bugs there. Bug reports are
> welcome in the [issues](https://github.com/Rugaliz/Deus-Ex-Aerugo/issues).

## How to install

You need:
- **Deus Ex: Game of the Year Edition** from Steam or GOG, installed.
- A 64-bit **Windows 10/11** or **Linux** PC.
- A graphics card with **Vulkan 1.3**: almost any card from 2016 or later, with an up-to-date driver.

Then:
1. Download the file for your system from the [releases page](https://github.com/Rugaliz/Deus-Ex-Aerugo/releases):
   the `.zip` for Windows, the `.tar.gz` for Linux.
2. Open your Deus Ex folder, the one that contains `System` and `Maps`. On Steam: right-click Deus Ex in your library,
   then *Manage* → *Browse local files*. On GOG Galaxy: *More* → *Manage installation* → *Show folder*.
3. Unpack the download and copy **`deusex.exe`** (Windows) or **`deusex`** (Linux) into that folder.
4. Double-click it to play (on Linux you can also run `./deusex` in that folder). Windows may warn that the program is
   from an unknown publisher (it is not code-signed): choose *More info* → *Run anyway*.

Your settings and saves are kept in `%APPDATA%\deus-ex-aerugo` (Windows) or `~/.config/deus-ex-aerugo` (Linux). To
uninstall, delete the file you copied, and that folder if you want your saves gone too. That folder also holds
`deusex.log` (your graphics card, driver, the levels visited and any errors) and `deusex.previous.log` (the run
before): please attach them to bug reports. After a crash, attach `deusex.previous.log` if you started the game again.

Extras, from a command prompt in the game folder: `deusex --windowed` plays in a window, `deusex --size 1920x1080`
sets its size, and `deusex --import-save "Save/Save0001/<map>.dxs"` continues a game saved by the original.

## Mods

Make a folder called **`Mods`** inside your Deus Ex folder and put each mod in it, as a `.zip` file or as a folder.
Mods are applied in alphabetical order, and a later one wins over an earlier one (put a number in front of a name to
change the order). To switch a mod off without deleting it, start its name with `_`.

- **Mods made as Deus Ex packages** (`.utx` texture packs, `.u` script and model packages, `.dx` maps): replace the
  game's file of the same name, or add new ones. Settings a mod needs can come as its own `DeusEx.ini` or `User.ini`,
  which go on top of the game's.
- **Your own textures:** a PNG of any size, placed at `Textures/<Package>/<Group>/<Name>.png` inside the mod, replaces
  that texture. To get every texture of the game already named that way, run `deusex --export-textures <folder>` in
  the game folder, edit or upscale what you like, and put the result in `Mods`.
- **Your own models:** a model edited with the mod kit (below), placed at `Meshes/<Package>/<Name>.aemesh` inside the
  mod, replaces that model's shape (for example `Meshes/DeusExItems/GEPGun.aemesh` for the GEP gun in your hands). The
  model keeps its animations, textures and everything the game does with it; only its vertices and faces change, in
  every animation frame. A replacement whose number of animation frames does not match the game's is left out (the
  game writes why in its log).

Mods that come with their own `.exe` or `.dll` files (launchers, renderers) do not apply to Aerugo. Big mods that
change the game's scripts should work too, but have not been tested yet.

### The mod kit

**`deusex-modkit`** (`deusex-modkit.exe` on Windows; it comes with Aerugo, put it next to `deusex`) shows, exports and
re-imports the game's models and textures. It reads your game folder and never writes into it.

- **`deusex-modkit`** opens the viewer: type in the search box and click a model (M) or texture (T) in the list (or
  browse with the arrow keys; what you pick opens at once), with filters for models and textures. A model plays its
  animations; the panel on the right lists its sequences, the textures the game gives it (click a character or item
  using it to switch skins) and its texture slots (click one to look at it). Drag with the mouse to look around,
  right-drag to turn around the model, wheel or W/A/S/D/Q/E to move. The buttons above the picture export (glTF, the
  current frame as OBJ, or a texture as PNG), open the export folder, play and step frames, show a weapon in your
  hands as the game places it, show the `Mods` folder's replacement, and mark back faces and open edges; every button
  has a key too. *Text -* / *Text +* (or Ctrl and the wheel) change the size of the text and buttons.
- **`deusex-modkit export <model>`** writes the model as **glTF 2.0** (Blender and most 3D tools open it) with its
  textures as PNG: every animation frame is a shape key (`frame_000`, ...) and every animation a clip of its own.
  `export-obj <model> --frame N` writes one frame as OBJ instead; `export-texture <texture>` writes a texture as PNG at
  the path a mod uses (`Textures/<Package>/<Group>/<Name>.png`).
- **`deusex-modkit import <file.glb> --mod Mods/MyMod`** turns the edited model back into a mod
  (`Mods/MyMod/Meshes/<Package>/<Name>.aemesh`). Keep the shape keys when exporting from Blender (glTF, *Shape Keys*
  on, the default). Vertices you add that do not move in any shape key follow the nearest original face in every
  frame, so new parts move with the model. A material's name gives its texture slot and surface (`slot0`,
  `slot1-masked`, as the export names them).

`deusex-modkit --help` lists everything (searching, pictures of a model from any angle, and more). The `mods` folder
of this repository has an example: [GEP gun widescreen](mods/gep-gun-widescreen/README.md), which completes the GEP
gun in your hands for wide screens (made with `export`, a Blender script and `import`).

## How to build it yourself

Install [Rust](https://rustup.rs) and a few build tools; then one script does the rest. The result lands in `dist/`;
copy it into your Deus Ex folder as in step 3 above.

**Windows:** install Rust, [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/) (choose
*Desktop development with C++*) and the [Vulkan SDK](https://vulkan.lunarg.com). Then, in PowerShell in this folder:

```
.\scripts\build-windows.ps1
```

**Linux:** install the packages for your distribution, then Rust, then run the build script in this folder.

Arch Linux:

```
sudo pacman -S rustup sdl3 shaderc cmake vulkan-icd-loader
rustup default stable
```

Ubuntu 22.04 / 24.04 (Ubuntu has no SDL3 package yet, so the script builds it into the game; that is what the
longer list is for):

```
sudo apt install build-essential curl cmake pkg-config glslang-tools spirv-tools \
  libx11-dev libxext-dev libxrandr-dev libxcursor-dev libxi-dev libxss-dev libxfixes-dev libxtst-dev \
  libxkbcommon-dev libwayland-dev wayland-protocols libdecor-0-dev libegl-dev libgl-dev libdrm-dev libgbm-dev \
  libasound2-dev libpulse-dev libpipewire-0.3-dev libdbus-1-dev libudev-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env
```

Then, on either:

```
scripts/build.sh
```

**Any Linux with Podman or Docker** (builds both the Linux and the Windows version; nothing else to install):

```
scripts/release.sh
```

The scripts can also copy the result for you: `scripts/build.sh --install "/path/to/Deus Ex"` or
`.\scripts\build-windows.ps1 -Install "C:\path\to\Deus Ex"`. A Windows version can be built on Linux too, with
`scripts/build-windows.sh` (needs MinGW-w64 and CMake).

<details>
<summary>For developers</summary>

Written in Rust, with Vulkan for graphics and SDL3 for the window, input and sound. Every engine function the game
relies on is ported from the original program and checked against it.

| Crate | What it does |
|---|---|
| `ue-package` | Unreal Engine 1 package files (`.u`, `.dx`, `.utx`, ...) |
| `ue-registry` | Classes, properties, functions, states and defaults of the script packages |
| `ue-assets` | Textures, palettes, meshes, sounds, music, fonts and levels |
| `ue-config` | Ini files and localised text, with a per-user layer that is the only one ever written |
| `ue-bsp` | Collision against the world |
| `ue-vm` | The UnrealScript virtual machine and the engine's native functions, physics, AI and UI |
| `dx-vis` | The original renderer's visibility pass |
| `dx-light` | Lightmaps, built from the maps' stored shadow data like the original at load time |
| `dx-fractal` | Procedural textures (water, fire, ice) |
| `dx-soft` | A CPU reference renderer used to check geometry and texture mapping |
| `dx-vk` | The Vulkan renderer |
| `dx-audio` | Sound decoding, the positional mixer, tracker music and the output device |
| `dx-input` | Key bindings, aliases and input processing |
| `dx-play` | The game executable |

`aerugo/` holds the engine's own small UnrealScript package (the extra settings menus), built into the executable.
Tests run against your game data: `DEUSEX_DIR="/path/to/Deus Ex" cargo test --release` (without `DEUSEX_DIR` the
tests that need it are skipped).

</details>

## Legal

MIT licensed (see `LICENSE`). Deus Ex is a trademark of its owners; this project is not affiliated with or endorsed by
them. It contains no code, scripts or assets of the original game: everything the game needs is read from your own
copy.
