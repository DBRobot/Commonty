; Included at the top of tauri's installer script: the installer keeps no
; file's time, so the same files make the same installer on any box
; (nix/app-release/windows.nix)
SetDateSave off

; the login entry the app writes for itself (app/src/background.rs) goes
; with it, but stays through an update
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Commonty"
  ${EndIf}
!macroend
