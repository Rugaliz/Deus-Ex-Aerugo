//=============================================================================
// MenuChoice_AerugoMaxDecals: how many of the newest decals are kept out of sight (AerugoSettings.MaxDecals).
// Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoMaxDecals extends MenuUIChoiceEnum;

var int counts[6];

function LoadSetting()
{
	local int i, best;

	best = 0;
	for (i = 0; i < 6; i++)
		if (counts[i] <= class'AerugoSettings'.default.MaxDecals)
			best = i;
	SetValue(best);
}

function SaveSetting()
{
	class'AerugoSettings'.default.MaxDecals = counts[int(GetValue())];
	class'AerugoSettings'.static.StaticSaveConfig();
}

function ResetToDefault()
{
	SetValue(defaultValue);
	SaveSetting();
}

defaultproperties
{
	counts(0)=0
	counts(1)=250
	counts(2)=500
	counts(3)=1000
	counts(4)=2000
	counts(5)=5000
	enumText(0)="Original"
	enumText(1)="250"
	enumText(2)="500"
	enumText(3)="1000"
	enumText(4)="2000"
	enumText(5)="5000"
	defaultValue=3
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="How many of the newest blood stains, bullet holes and scorch marks stay when out of sight. Original: they fade after about 20 seconds unseen."
	actionText="&Kept Decals"
}
