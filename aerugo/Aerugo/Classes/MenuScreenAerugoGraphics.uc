//=============================================================================
// MenuScreenAerugoGraphics: the port's picture options, on the Display screen's art. Our own code (MIT).
//=============================================================================
class MenuScreenAerugoGraphics expands MenuScreenDisplay;

defaultproperties
{
	choices(0)=Class'MenuChoice_AerugoTextureFilter'
	choices(1)=Class'MenuChoice_AerugoAntiAliasing'
	choices(2)=Class'MenuChoice_AerugoColorDepth'
	choices(3)=Class'MenuChoice_AerugoLightmaps'
	choices(4)=Class'MenuChoice_AerugoMeshLighting'
	choices(5)=Class'MenuChoice_AerugoRTShadows'
	choices(6)=Class'MenuChoice_AerugoRTAO'
	choices(7)=Class'MenuChoice_AerugoMaxDecals'
	choices(8)=Class'MenuChoice_AerugoWideView'
	choices(9)=None
	choices(10)=None
	choices(11)=None
	choices(12)=None
	Title="Advanced Graphics"
}
