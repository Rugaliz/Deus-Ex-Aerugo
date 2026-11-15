//=============================================================================
// MenuChoice_AerugoMouseDPI: the mouse's DPI (AerugoSettings.MouseDPI). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoMouseDPI extends MenuUIChoiceEnum;

var int Dpis[9];

function LoadSetting()
{
	local int i;

	for (i = 0; i < arrayCount(Dpis); i++)
		if (Dpis[i] == class'AerugoSettings'.default.MouseDPI)
		{
			SetValue(i);
			return;
		}
	SetValue(defaultValue);
}

function SaveSetting()
{
	class'AerugoSettings'.default.MouseDPI = Dpis[int(GetValue())];
	class'AerugoSettings'.static.StaticSaveConfig();
}

function ResetToDefault()
{
	SetValue(defaultValue);
	SaveSetting();
}

defaultproperties
{
	Dpis(0)=400
	Dpis(1)=800
	Dpis(2)=1000
	Dpis(3)=1200
	Dpis(4)=1600
	Dpis(5)=2400
	Dpis(6)=3200
	Dpis(7)=4000
	Dpis(8)=6400
	enumText(0)="400"
	enumText(1)="800"
	enumText(2)="1000"
	enumText(3)="1200"
	enumText(4)="1600"
	enumText(5)="2400"
	enumText(6)="3200"
	enumText(7)="4000"
	enumText(8)="6400"
	defaultValue=1
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="Your mouse's DPI (see its software). Movement is scaled so Mouse Sensitivity means the same speed on any mouse."
	actionText="Mouse |&DPI"
}
