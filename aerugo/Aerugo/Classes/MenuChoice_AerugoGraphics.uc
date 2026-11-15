//=============================================================================
// MenuChoice_AerugoGraphics: opens the Advanced Graphics screen. Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoGraphics extends MenuUIChoiceAction;

defaultproperties
{
	Action=MA_MenuScreen
	Invoke=Class'MenuScreenAerugoGraphics'
	HelpText="Texture filtering, anti-aliasing, color depth and lighting."
	actionText="Advanced |&Graphics..."
}
