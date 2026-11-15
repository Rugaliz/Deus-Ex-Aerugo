//=============================================================================
// MenuChoice_AerugoMeshLighting: how characters and objects are lit (AerugoSettings.PixelLighting). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoMeshLighting extends MenuUIChoiceEnum;

function LoadSetting()
{
	SetValue(class'AerugoSettings'.default.PixelLighting);
}

function SaveSetting()
{
	class'AerugoSettings'.default.PixelLighting = int(GetValue());
	class'AerugoSettings'.static.StaticSaveConfig();
}

function ResetToDefault()
{
	SetValue(defaultValue);
	SaveSetting();
}

defaultproperties
{
	enumText(0)="Per Vertex"
	enumText(1)="Per Pixel"
	defaultValue=1
	defaultInfoWidth=98
	defaultInfoPosX=270
	HelpText="Per Pixel: lights fall smoothly across characters and objects. Per Vertex: as the original lit them."
	actionText="|&Mesh Lighting"
}
