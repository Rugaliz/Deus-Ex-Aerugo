//=============================================================================
// MenuChoice_AerugoVSync: vertical sync (AerugoSettings.VSync). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoVSync extends MenuUIChoiceEnum;

function LoadSetting()
{
	SetValue(class'AerugoSettings'.default.VSync);
}

function SaveSetting()
{
	class'AerugoSettings'.default.VSync = int(GetValue());
	class'AerugoSettings'.static.StaticSaveConfig();
}

function ResetToDefault()
{
	SetValue(defaultValue);
	SaveSetting();
}

defaultproperties
{
	enumText(0)="Off"
	enumText(1)="On"
	enumText(2)="Adaptive"
	enumText(3)="Fast"
	defaultValue=1
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="Off: lowest latency, may tear. On: no tearing, capped at the refresh rate (use with FreeSync/G-Sync and an Auto frame limit). Adaptive: on, tears when slower. Fast: no tearing, not capped."
	actionText="|&Vertical Sync"
}
