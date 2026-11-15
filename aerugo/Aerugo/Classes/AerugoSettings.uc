//=============================================================================
// AerugoSettings: the port's own settings, kept in Aerugo.ini. Read by the port's host (vertical sync, frame rate
// limit, sound output rate, mouse DPI); written by the menu choices of this package.
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

defaultproperties
{
	VSync=1
	FrameLimit=0
	OutputRate=0
	MouseDPI=800
}
