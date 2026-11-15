//=============================================================================
// MenuChoice_AerugoSpacer: an empty row, so the choices after it stay on the rows the screen's art draws for them
// (the Sound screen's fourth slider row, once its effect channels choice is gone). Our own code (MIT).
//=============================================================================
class MenuChoice_AerugoSpacer extends MenuUIChoice;

// no button: an empty row that cannot be selected (NewChild shows the window after InitWindow, so it cannot hide)
function CreateActionButton()
{
}

function LoadSetting()
{
}

function SaveSetting()
{
}

function ResetToDefault()
{
}
