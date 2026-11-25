//=============================================================================
// AerugoCreditsWindow: the game's credits with a section for Deus Ex Aerugo after them, before the team photo that
// ends them. The port opens it wherever the game opens CreditsWindow (the main menu, the endings). Our own code (MIT).
//=============================================================================
class AerugoCreditsWindow extends CreditsWindow;

function ProcessFinished()
{
	PrintLn();
	PrintLn();
	PrintHeader("Deus Ex Aerugo");
	PrintLn();
	PrintHeader("Developed by");
	PrintText("Ricardo Rugaliz Fernandes");
	PrintLn();
	PrintHeader("Playtested by");
	PrintText("Leandro Orochi Fernandes");
	Super.ProcessFinished();
}
