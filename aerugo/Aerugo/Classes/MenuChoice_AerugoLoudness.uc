//=============================================================================
// MenuChoice_AerugoLoudness: the loudness control on the sound effects (AerugoSettings.Loudness). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoLoudness extends MenuUIChoiceEnum;

function LoadSetting()
{
	if (class'AerugoSettings'.default.Loudness != 0)
		SetValue(1);
	else
		SetValue(0);
}

function SaveSetting()
{
	class'AerugoSettings'.default.Loudness = int(GetValue());
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
	enumText(1)="Even"
	defaultValue=1
	HelpText="Even keeps many sounds playing at once from getting too loud. Original mixes them as the original game."
	actionText="|&Loudness"
}
