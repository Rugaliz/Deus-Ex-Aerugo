//=============================================================================
// MenuChoice_AerugoAntiAliasing: multisample anti-aliasing (AerugoSettings.AntiAliasing). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoAntiAliasing extends MenuUIChoiceEnum;

var int Values[4];

function LoadSetting()
{
	local int i;

	for (i = 0; i < arrayCount(Values); i++)
		if (Values[i] == class'AerugoSettings'.default.AntiAliasing)
		{
			SetValue(i);
			return;
		}
	SetValue(defaultValue);
}

function SaveSetting()
{
	class'AerugoSettings'.default.AntiAliasing = Values[int(GetValue())];
	class'AerugoSettings'.static.StaticSaveConfig();
}

function ResetToDefault()
{
	SetValue(defaultValue);
	SaveSetting();
}

defaultproperties
{
	Values(0)=1
	Values(1)=2
	Values(2)=4
	Values(3)=8
	enumText(0)="Off"
	enumText(1)="2x"
	enumText(2)="4x"
	enumText(3)="8x"
	defaultValue=2
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="Smooths jagged edges of geometry, and of grates and fences. Higher is smoother and slower."
	actionText="|&Anti-Aliasing"
}
