//=============================================================================
// MenuChoice_AerugoFOV: the field of view, through the game's own SetDesiredFOV (saved as DefaultFOV in User.ini). The class default is set too, so a scope or binoculars restore this one. Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoFOV extends MenuUIChoiceEnum;

var int Angles[9];

function LoadSetting()
{
	local int i, best;

	best = 0;
	for (i = 1; i < arrayCount(Angles); i++)
		if (Abs(Angles[i] - player.DefaultFOV) < Abs(Angles[best] - player.DefaultFOV))
			best = i;
	SetValue(best);
}

function SaveSetting()
{
	player.SetDesiredFOV(Angles[int(GetValue())]);
	player.SetFOVAngle(player.DefaultFOV);
	class'DeusExPlayer'.default.DefaultFOV = player.DefaultFOV;
	class'DeusExPlayer'.default.DesiredFOV = player.DefaultFOV;
	class'JCDentonMale'.default.DefaultFOV = player.DefaultFOV;
	class'JCDentonMale'.default.DesiredFOV = player.DefaultFOV;
}

function ResetToDefault()
{
	SetValue(defaultValue);
	SaveSetting();
}

defaultproperties
{
	Angles(0)=75
	Angles(1)=80
	Angles(2)=85
	Angles(3)=90
	Angles(4)=95
	Angles(5)=100
	Angles(6)=105
	Angles(7)=110
	Angles(8)=120
	enumText(0)="75"
	enumText(1)="80"
	enumText(2)="85"
	enumText(3)="90"
	enumText(4)="95"
	enumText(5)="100"
	enumText(6)="105"
	enumText(7)="110"
	enumText(8)="120"
	defaultValue=0
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="How wide the view is, in degrees across a 4:3 picture (wider screens see more to the sides). 75 is the original's."
	actionText="Field |&of View"
}
