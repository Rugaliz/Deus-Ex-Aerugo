//=============================================================================
// MenuChoice_AerugoColorDepth: the scene drawn with 8 or 16 bits per channel (AerugoSettings.DeepColor). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoColorDepth extends MenuUIChoiceEnum;

function LoadSetting()
{
	SetValue(class'AerugoSettings'.default.DeepColor);
}

function SaveSetting()
{
	class'AerugoSettings'.default.DeepColor = int(GetValue());
	class'AerugoSettings'.static.StaticSaveConfig();
}

function ResetToDefault()
{
	SetValue(defaultValue);
	SaveSetting();
}

defaultproperties
{
	enumText(0)="8-bit"
	enumText(1)="16-bit"
	defaultValue=1
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="16-bit: smooth fog and dark gradients without banding. 8-bit: as the original drew them."
	actionText="|&Color Depth"
}
