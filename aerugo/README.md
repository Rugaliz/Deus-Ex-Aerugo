# Aerugo: the port's own script package

`aerugo/Aerugo/Classes/*.uc` is UnrealScript of our own (MIT): menu choices for the settings the port adds and the
config class that holds them. It is compiled against the retail packages with the SDK's UCC and kept as
`aerugo/Aerugo.u`, which the port builds into its binary (`ue_registry::BUILTIN_PACKAGES`) and loads after the game's
packages (a `System/Aerugo.u` in the game folder takes its place, for development).

Rebuilding it needs UCC from the official Deus Ex SDK (version 1112f; a Windows program, Wine works) and a scratch
copy of the game, never your real installation: put the classes in `Aerugo/Classes/` of that copy, add
`EditPackages=Aerugo` after `EditPackages=DeusEx` in its `System/DeusEx.ini`, run `ucc make` in its `System/` and
copy the new `System/Aerugo.u` here.

## What it adds

`AerugoSettings` (config `Aerugo.ini`, section `[Aerugo.AerugoSettings]`, in the port's config folder):

| Key | Values | Default |
|---|---|---|
| `VSync` | 0 off (immediate), 1 on (FIFO), 2 adaptive (relaxed FIFO), 3 fast (mailbox) | 1 |
| `FrameLimit` | frames per second; 0 none, -1 the display's refresh rate | 0 |
| `OutputRate` | sound output in Hz; 0 the system's (48000 when unknown, never below 44100) | 0 |
| `MouseDPI` | the mouse's counts per inch; movement is scaled to the 400 DPI the game was tuned for | 800 |
| `Loudness` | 1 even: a compressor on the sound effects keeps many sounds at once (a room full of machines) from getting too loud; quiet scenes are as the original mixes them; 0 the original's mix | 1 |
| `DialogueKeys` | in a conversation showing choices, the number keys `1`..`9` and `0` pick that choice; 0 as the original (mouse, arrow keys and Enter) | 1 |
| `MaxDecals` | how many of the newest decals (blood, bullet holes, scorch marks) stay out of sight; older ones go as in the original (unseen for a second, some 20 s after they appeared); 0 as the original | 1000 |

Menu choices (`MenuChoice_Aerugo*`): vertical sync, frame rate limit, mouse DPI, output rate, loudness, a mouse sensitivity
slider in half steps from 0.5 to 10 labelled with the mouse travel of a full turn (54.8 cm / sensitivity once the
DPI is right), and an empty row.

The port (`Vm::apply_port_menus`) lists them in the stock screens' `choices`: Display Settings shows brightness,
full screen, resolution, vertical sync, frame rate limit and mouse DPI (no render device, texture depth, texture
detail, detail textures or decals: the port always draws at the best quality); Sound Settings shows the three
volumes, an empty row (the art's fourth slider), output rate, reverse stereo and surround (effect channels are fixed
at 128, sound quality and 3D hardware are gone); Controls has the new sensitivity slider.
