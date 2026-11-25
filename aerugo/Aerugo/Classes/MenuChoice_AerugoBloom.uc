//=============================================================================
// MenuChoice_AerugoBloom: a soft glow around the brightest parts of the picture (AerugoSettings.Bloom). Our own code
// (MIT).
//=============================================================================
class MenuChoice_AerugoBloom extends MenuUIChoiceEnum;

function LoadSetting()
{
	if (class'AerugoSettings'.default.Bloom != 0)
		SetValue(1);
	else
		SetValue(0);
}

function SaveSetting()
{
	class'AerugoSettings'.default.Bloom = int(GetValue());
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
	enumText(1)="Subtle"
	defaultValue=0
	HelpText="A soft glow around lamps, light strips and other bright parts of the picture. Off draws as the original game."
	actionText="|&Bloom"
}
