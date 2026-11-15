# Deus Ex Aerugo

An open-source engine for **Deus Ex** (2000, the 1112fm Game of the Year build) written in Rust. It runs the game's
original UnrealScript bytecode in its own virtual machine, draws with Vulkan and uses SDL3 for the window, input and
sound. The goal is to behave exactly like the original executable, one engine function at a time, on current
systems: Linux and Windows, any resolution and aspect ratio, high refresh rates.

**No game data is included.** You need your own copy of Deus Ex (Steam or GOG). The engine reads the game's
`System/`, `Maps/`, `Textures/`, `Sounds/` and `Music/` folders and never writes into them.

> **Status: work in progress, not yet a complete game.** The logo cutscene, the main menu and New Game work; the
> training mission and Liberty Island are playable with the HUD, inventory, skills, augmentations, goals, combat,
> conversations, DataLinks, computers, doors, level changes and saving/loading (saves of the original game can be
> imported). No mission has been played through to the end yet, and fog, particle effects, some procedural textures,
> many augmentation effects, swimming and the endings are missing or incomplete.

## Playing

1. Download the archive for your system from the [releases](https://github.com/Rugaliz/Deus-Ex-Aerugo/releases).
2. Copy `deusex` (Linux) or `deusex.exe` (Windows) into your Deus Ex folder, the one that contains `System/` and
   `Maps/`. On Steam that is `steamapps/common/Deus Ex`.
3. Start it from there (on Linux: `./deusex`). It plays the logo; a click or Esc opens the main menu.

You need a graphics driver with **Vulkan 1.3** (any GPU from roughly 2016 on with a current driver). Nothing else has
to be installed: SDL3 is built into the executable.

Command line (all optional):

```
deusex ["<Deus Ex folder>"] [<Map>] [--windowed] [--size 1920x1080]
deusex 00_Training                     # start a map directly
```

Settings, key bindings and saved games are kept in your user folder, never in the game folder:
`~/.config/deus-ex-aerugo/` on Linux, `%APPDATA%\deus-ex-aerugo\` on Windows.

The settings menus are the game's own, with a few changes for current hardware: texture and detail options are gone
(the best quality is always used), and there are vertical sync modes (off, on, adaptive, fast), a frame rate limit,
the mouse's DPI with a sensitivity slider labelled in centimetres per full turn, and the sound output rate (the
system's by default).

## Building from source

All builds need a Rust toolchain ([rustup](https://rustup.rs)) and a shader compiler: `glslc` (shaderc, or the
Vulkan SDK) or `glslangValidator` (glslang).

| Build | Command | Needs |
|---|---|---|
| Linux, for this machine | `scripts/build.sh` → `dist/deusex` | SDL3 (`pacman -S rustup sdl3 shaderc vulkan-icd-loader`) |
| Windows, from Linux | `scripts/build-windows.sh` → `dist/deusex.exe` | MinGW-w64 and CMake (`pacman -S mingw-w64-gcc cmake`) |
| Windows, on Windows | `.\scripts\build-windows.ps1` → `dist\deusex.exe` | Visual Studio 2022 Build Tools (C++), the Vulkan SDK |
| Release archives | `scripts/release.sh` → `dist/*.tar.gz`, `dist/*.zip` | Podman or Docker |

`scripts/build.sh --install "<Deus Ex folder>"` and `.\scripts\build-windows.ps1 -Install "<Deus Ex folder>"` also
copy the executable there. The release build runs in an Ubuntu 22.04 container (`packaging/Containerfile`), so the
Linux executable works on any distribution with glibc 2.35 or newer; SDL3 is compiled from source and linked into
both executables.

The test suite runs against your game data:

```
DEUSEX_DIR="/path/to/Deus Ex" cargo test --release
```

Without `DEUSEX_DIR` the tests that need game data are skipped.

## How it is organised

| Crate | What it does |
|---|---|
| `ue-package` | Unreal Engine 1 package files (`.u`, `.dx`, `.utx`, ...): header, name, import and export tables |
| `ue-registry` | Classes, properties, functions, states and defaults of the script packages |
| `ue-assets` | Textures, palettes, meshes, sounds, music, fonts and levels |
| `ue-config` | Ini files and localised text, with a per-user layer that is the only one ever written |
| `ue-bsp` | Collision against the world: point regions, line and box traces |
| `ue-vm` | The UnrealScript virtual machine and the engine's native functions, physics, AI and UI |
| `dx-vis` | The original renderer's visibility pass (span buffers, front-to-back BSP occlusion) |
| `dx-light` | Lightmaps, built from the maps' stored shadow data and light actors like the original at load time |
| `dx-fractal` | Procedural textures (water, fire, ice) generated every frame |
| `dx-soft` | A CPU reference renderer used to check geometry and texture mapping |
| `dx-vk` | The Vulkan renderer |
| `dx-audio` | Sound decoding, the positional mixer, tracker music and the output device |
| `dx-input` | Key bindings, aliases and input processing |
| `dx-play` | The game executable: window, main loop, frame pacing, input, audio and rendering together |

`aerugo/` holds the engine's own small UnrealScript package (the extra settings menus), built into the executable.

## Legal

MIT licensed (see `LICENSE`). Deus Ex is a trademark of its owners; this project is not affiliated with or endorsed by
them. It contains no code, scripts or assets of the original game: everything the game needs is read from your own
copy at run time.
