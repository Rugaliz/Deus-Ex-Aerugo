//=============================================================================
// MenuChoice_AerugoMouseSpeed: Mouse Sensitivity in half steps from 0.5 to 10, each labelled with the mouse travel of a
// full turn in cm (the port scales movement to 400 DPI: 54.8 cm / sensitivity). The slider's ticks are whole numbers
// (1 to 20, SetTicks takes integers): the sensitivity is half the tick value. Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoMouseSpeed extends MenuChoice_MouseSensitivity;

function SetEnumerators()
{
	local int i;
	local float s;

	for (i = 0; i < numTicks; i++)
	{
		s = (i + 1) * 0.5;
		SetEnumeration(i, int(54.8 / s + 0.5) $ "cm");
	}
}

function LoadSetting()
{
	saveValue = player.MouseSensitivity;
	SetValue(FClamp(player.MouseSensitivity * 2, startValue, endValue));
}

function ResetToDefault()
{
	player.UpdateSensitivity(defaultValue * 0.5);
	SetValue(defaultValue);
}

event bool ScalePositionChanged(Window scale, int newTickPosition, float newValue, bool bFinal)
{
	if (bInitializing)
		return False;
	player.UpdateSensitivity(GetValue() * 0.5);
	return false;
}

defaultproperties
{
	numTicks=20
	startValue=1.0
	endValue=20.0
	defaultValue=6.0
	HelpText="Turn speed, shown as how far the mouse travels for a full turn (set Mouse DPI in Display Settings to your mouse's)."
}
