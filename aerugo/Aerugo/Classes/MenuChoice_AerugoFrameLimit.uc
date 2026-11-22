//=============================================================================
// MenuChoice_AerugoFrameLimit: frame rate limit (AerugoSettings.FrameLimit). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoFrameLimit extends MenuUIChoiceEnum;

var int Limits[9];

function LoadSetting()
{
	local int i;

	for (i = 0; i < arrayCount(Limits); i++)
		if (Limits[i] == class'AerugoSettings'.default.FrameLimit)
		{
			SetValue(i);
			return;
		}
	SetValue(0);
}

function SaveSetting()
{
	class'AerugoSettings'.default.FrameLimit = Limits[int(GetValue())];
	class'AerugoSettings'.static.StaticSaveConfig();
}

function ResetToDefault()
{
	SetValue(defaultValue);
	SaveSetting();
}

defaultproperties
{
	Limits(0)=0
	Limits(1)=-1
	Limits(2)=30
	Limits(3)=60
	Limits(4)=90
	Limits(5)=120
	Limits(6)=144
	Limits(7)=165
	Limits(8)=240
	enumText(0)="Off"
	enumText(1)="Auto"
	enumText(2)="30"
	enumText(3)="60"
	enumText(4)="90"
	enumText(5)="120"
	enumText(6)="144"
	enumText(7)="165"
	enumText(8)="240"
	defaultValue=0
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="Caps the frame rate. Auto: the display's refresh rate."
	actionText="|&Frame Rate Limit"
}
