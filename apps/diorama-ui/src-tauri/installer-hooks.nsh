; Diorama's additions to Tauri's NSIS installer (tauri.bundle.conf.json).
; The installer closes diorama-ui itself; dioramad and its web hosts run on
; their own, so they are stopped here before their files are replaced or
; removed. Only processes started from this install are touched.

!macro DIORAMA_STOP_DAEMON
  ; Close the UI first (it would start dioramad again), then ask the daemon to
  ; quit (it ends its web hosts), then make sure.
  IfFileExists "$INSTDIR\diorama-ui.exe" 0 +3
    nsExec::Exec '"$INSTDIR\diorama-ui.exe" --quit'
    Pop $0
  IfFileExists "$INSTDIR\diorama.exe" 0 +3
    nsExec::Exec '"$INSTDIR\diorama.exe" quit'
    Pop $0
  nsExec::Exec `powershell -NoProfile -NonInteractive -Command "Start-Sleep -Milliseconds 800; Get-Process diorama-ui, dioramad, diorama-webhost -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like '$INSTDIR\*' } | Stop-Process -Force"`
  Pop $0
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro DIORAMA_STOP_DAEMON
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro DIORAMA_STOP_DAEMON
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Updates reinstall over the old version without uninstalling, so this
  ; only runs when Diorama is really removed: drop its sign-in entry.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Diorama"
  ; "Delete the application data" ticked: Diorama's settings and caches. The
  ; caches sit in the install folder (%LOCALAPPDATA%\Diorama), which Tauri's
  ; uninstaller only removes when empty.
  ${If} $DeleteAppDataCheckboxState = 1
    RMDir /r "$APPDATA\Diorama"
    RMDir /r "$LOCALAPPDATA\Diorama\cache"
    RMDir /r "$LOCALAPPDATA\Diorama\webview"
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
