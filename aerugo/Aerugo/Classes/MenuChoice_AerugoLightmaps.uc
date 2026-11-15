//=============================================================================
// MenuChoice_AerugoLightmaps: lightmap filtering (AerugoSettings.SmoothLightmaps). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoLightmaps extends MenuUIChoiceEnum;

function LoadSetting()
{
	SetValue(class'AerugoSettings'.default.SmoothLightmaps);
}

function SaveSetting()
{
	class'AerugoSettings'.default.SmoothLightmaps = int(GetValue());
	class'AerugoSettings'.static.StaticSaveConfig();
}

function ResetToDefault()
{
	SetValue(defaultValue);
	SaveSetting();
}

defaultproperties
{
	enumText(0)="Original"
	enumText(1)="Smooth"
	defaultValue=1
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="Smooth: softer, rounder shadow edges on walls and floors. Original: as the game drew them."
	actionText="|&Lightmap Filtering"
}
