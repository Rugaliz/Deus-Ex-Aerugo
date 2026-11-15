//=============================================================================
// MenuChoice_AerugoRTAO: ray-traced ambient occlusion (AerugoSettings.RTAO). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoRTAO extends MenuUIChoiceEnum;

function LoadSetting()
{
	SetValue(class'AerugoSettings'.default.RTAO);
}

function SaveSetting()
{
	class'AerugoSettings'.default.RTAO = int(GetValue());
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
	HelpText="Soft contact shadows where walls, floors and objects meet. Needs a graphics card with ray tracing."
	actionText="Ambient |&Occlusion"
}
