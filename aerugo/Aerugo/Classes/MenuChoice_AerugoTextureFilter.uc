//=============================================================================
// MenuChoice_AerugoTextureFilter: texture filtering (AerugoSettings.TextureFilter). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoTextureFilter extends MenuUIChoiceEnum;

function LoadSetting()
{
	SetValue(class'AerugoSettings'.default.TextureFilter);
}

function SaveSetting()
{
	class'AerugoSettings'.default.TextureFilter = int(GetValue());
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
	enumText(1)="Trilinear"
	enumText(2)="2x"
	enumText(3)="4x"
	enumText(4)="8x"
	enumText(5)="16x"
	defaultValue=5
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="How textures are smoothed at a distance and at an angle. Original: as the game drew them in 2000. 2x to 16x: trilinear with anisotropic filtering, sharpest at 16x."
	actionText="Texture |&Filtering"
}
