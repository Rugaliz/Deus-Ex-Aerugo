//=============================================================================
// MenuChoice_AerugoRTShadows: ray-traced shadows of characters, objects and doors (AerugoSettings.RTShadows). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoRTShadows extends MenuUIChoiceEnum;

function LoadSetting()
{
	SetValue(class'AerugoSettings'.default.RTShadows);
}

function SaveSetting()
{
	class'AerugoSettings'.default.RTShadows = int(GetValue());
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
	defaultValue=1
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="Characters, objects and doors cast shadows from the map's own lights, on the world and on each other. Needs a graphics card with ray tracing."
	actionText="Ray-Traced |&Shadows"
}
