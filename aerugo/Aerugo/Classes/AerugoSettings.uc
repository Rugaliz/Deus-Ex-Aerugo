//=============================================================================
// AerugoSettings: the port's own settings, kept in Aerugo.ini. Read by the port's host (vertical sync, frame rate
// limit, sound output rate, mouse DPI, picture options); written by the menu choices of this package.
// Our own code (MIT), compiled against the retail packages with the SDK's UCC.
//=============================================================================
class AerugoSettings extends Object config(Aerugo);

// 0 off (immediate), 1 on (FIFO), 2 adaptive (relaxed FIFO), 3 fast (mailbox)
var config int VSync;
// frames per second; 0 none, -1 the display's refresh rate minus 3 (variable refresh rate displays)
var config int FrameLimit;
// sound output rate in Hz; 0 the system's (48000 when unknown, never below 44100)
var config int OutputRate;
// the mouse's counts per inch: movement is scaled to the 400 DPI mice the game was tuned for
var config int MouseDPI;
// picture options (the Advanced Graphics screen): texture filtering 0 original, 1 trilinear, 2..5 anisotropic
// 2x..16x; multisampling 1, 2, 4, 8; 16-bit scene; bicubic lightmaps; per-pixel mesh lighting
var config int TextureFilter;
var config int AntiAliasing;
var config int DeepColor;
var config int SmoothLightmaps;
var config int PixelLighting;
// ray-traced shadows of objects and ambient occlusion (ignored without a graphics card that can trace rays)
var config int RTShadows;
var config int RTAO;
// how many of the newest decals (blood, bullet holes, scorch marks) stay however long they are out of sight; older
// ones go as in the original (a decal not seen for a second goes some 20 s after it appeared); 0 as the original
var config int MaxDecals;
// the loudness control on the sound effects: 1 even (many sounds at once do not get too loud), 0 the original's mix
var config int Loudness;
// in a conversation showing choices, the number keys 1..9 and 0 pick that choice: 1 on, 0 as the original (mouse,
// arrow keys and Enter only)
var config int DialogueKeys;
// the field of view on wide screens: 0 as the original (the angle spans the whole screen width), 1 wider (the angle is
// for a 4:3 picture and wider screens see more at the sides, "Hor+")
var config int WideView;

defaultproperties
{
	VSync=1
	FrameLimit=0
	OutputRate=0
	MouseDPI=800
	TextureFilter=5
	AntiAliasing=4
	DeepColor=1
	SmoothLightmaps=1
	PixelLighting=1
	RTShadows=1
	RTAO=1
	MaxDecals=1000
	Loudness=1
	DialogueKeys=1
	WideView=0
}
