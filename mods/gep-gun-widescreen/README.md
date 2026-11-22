# GEP gun widescreen

A mesh mod for Deus Ex Aerugo: the GEP gun in your hands (`DeusExItems.GEPGun`) made whole for wide screens.

The first-person GEP gun was modelled for a 4:3 screen: its right side ends right at the old screen edge. On 16:9 and
21:9 screens (Aerugo widens the view instead of cutting it), that edge is in the middle of the picture, and the rear
right corner of the gun shows as a thin ledge sticking out over nothing. This mod puts the block under that ledge:
the gun's riveted rear plate carries on to the ledge's outer corner (same texture, continuing seamlessly), with the
block's outer side and bottom. Six triangles and two vertices; they move with the gun in every animation (select,
fire, reload, idle), and the gun keeps its own texture, animations and sounds.

Install: put `GEPGunWidescreen.zip` (or the `GEPGunWidescreen` folder) into the `Mods` folder of your Deus Ex folder.
To switch it off, rename it to start with `_`.

Made with the mod kit (`deusex-modkit`) and Blender: `build.sh` makes it from your own copy of the game.
