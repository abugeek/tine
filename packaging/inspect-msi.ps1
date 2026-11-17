param([string]$Msi = "D:\projects\rust\tine-wt\T23\packaging\out\tine-0.1.0-x64.msi")
# Dev tool: dump the interesting tables of the built MSI (T23 review aid).
$wi = New-Object -ComObject WindowsInstaller.Installer
$db = $wi.GetType().InvokeMember('OpenDatabase','InvokeMethod',$null,$wi,@($Msi,0))
function Dump($q, $n) {
  Write-Host "== $n =="
  $v = $db.GetType().InvokeMember('OpenView','InvokeMethod',$null,$db,@($q))
  $v.GetType().InvokeMember('Execute','InvokeMethod',$null,$v,$null) | Out-Null
  while ($true) {
    $r = $v.GetType().InvokeMember('Fetch','InvokeMethod',$null,$v,$null)
    if (-not $r) { break }
    $line = @()
    for ($i=1; $i -le 8; $i++) {
      try { $line += $r.GetType().InvokeMember('StringData','GetProperty',$null,$r,@($i)) } catch {}
    }
    Write-Host ($line -join ' | ')
  }
}
Dump 'SELECT `Property`, `Value` FROM `Property`' "Property (all)"
Dump 'SELECT `Registry`, `Root`, `Key`, `Name`, `Value` FROM `Registry`' "Registry"
Dump "SELECT Action, Type, Source, Target FROM CustomAction" "CustomAction"
Dump "SELECT Dialog_, Control_, Event, Argument, Condition FROM ControlEvent WHERE Dialog_='ExitDialog'" "ControlEvent (ExitDialog)"
Dump "SELECT Directory, Directory_Parent FROM Directory" "Directory"
Dump "SELECT File, Component_, FileSize FROM File" "File"
Dump "SELECT Component, Directory_, Attributes FROM Component" "Component"
Dump "SELECT Action, Condition, Sequence FROM InstallExecuteSequence" "InstallExecuteSequence"
Dump "SELECT Shortcut, Directory_, Target FROM Shortcut" "Shortcut"
Dump "SELECT CreateFolder FROM CreateFolder" "CreateFolder"
