Option Explicit

Dim shell, files, workspace, launcher, command, result
Set shell = CreateObject("WScript.Shell")
Set files = CreateObject("Scripting.FileSystemObject")
workspace = files.GetParentFolderName(WScript.ScriptFullName)
launcher = files.BuildPath(workspace, "tools\start-spindle.ps1")
command = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File " & Chr(34) & launcher & Chr(34) & " -BuildIfMissing"
result = shell.Run(command, 0, True)
If result <> 0 Then
    MsgBox "Spindle could not start. Open Start Spindle.cmd for diagnostics.", vbExclamation, "Spindle"
End If
