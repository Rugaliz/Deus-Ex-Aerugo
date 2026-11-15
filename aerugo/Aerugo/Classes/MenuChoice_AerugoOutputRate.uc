//=============================================================================
// MenuChoice_AerugoOutputRate: sound output rate (AerugoSettings.OutputRate); the restart message of the stock sample
// rate choice. Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoOutputRate extends MenuChoice_SampleRate;

var int Rates[5];

function LoadSetting()
{
	local int i;

	for (i = 0; i < arrayCount(Rates); i++)
		if (Rates[i] == class'AerugoSettings'.default.OutputRate)
		{
			SetValue(i);
			return;
		}
	SetValue(0);
}

function SaveSetting()
{
	class'AerugoSettings'.default.OutputRate = Rates[int(GetValue())];
	class'AerugoSettings'.static.StaticSaveConfig();
}

function ResetToDefault()
{
	SetValue(defaultValue);
	SaveSetting();
}

defaultproperties
{
	Rates(0)=0
	Rates(1)=44100
	Rates(2)=48000
	Rates(3)=96000
	Rates(4)=192000
	enumText(0)="Auto"
	enumText(1)="44100Hz"
	enumText(2)="48000Hz"
	enumText(3)="96000Hz"
	enumText(4)="192000Hz"
	defaultValue=0
	HelpText="Auto uses the system's output rate (48000Hz when it cannot be read, never below 44100Hz)."
	actionText="Output |&Rate"
}
