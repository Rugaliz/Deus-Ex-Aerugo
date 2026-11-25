//=============================================================================
// MenuChoice_AerugoWideView: the field of view on wide screens (AerugoSettings.WideView). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoWideView extends MenuUIChoiceEnum;

function LoadSetting()
{
	if (class'AerugoSettings'.default.WideView != 0)
		SetValue(1);
	else
		SetValue(0);
}

function SaveSetting()
{
	class'AerugoSettings'.default.WideView = int(GetValue());
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
	enumText(1)="Wider"
	defaultValue=0
	HelpText="Original spreads the field of view across the whole screen, as the original game. Wider keeps a 4:3 screen's view and shows more at the sides on wide screens."
	actionText="Wide Screen |&View"
}
