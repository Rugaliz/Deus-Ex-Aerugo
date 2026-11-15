//=============================================================================
// MenuChoice_AerugoHUDFilter: filtering of the scaled HUD and menu art (AerugoSettings.SmoothHUD). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoHUDFilter extends MenuUIChoiceEnum;

function LoadSetting()
{
	SetValue(class'AerugoSettings'.default.SmoothHUD);
}

function SaveSetting()
{
	class'AerugoSettings'.default.SmoothHUD = int(GetValue());
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
	enumText(1)="Crisp"
	enumText(2)="Smooth"
	defaultValue=2
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="How the HUD, menus and screens look when scaled up. Smooth: filtered, no jagged pixels. Crisp: sharp pixels of even size. Original: as the game drew them."
	actionText="|&HUD Filtering"
}
