; Lumen's additions to Tauri's NSIS installer (tauri.bundle.conf.json).
; The installer closes lumen-ui itself; lumend and its web hosts run on
; their own, so they are stopped here before their files are replaced or
; removed. Only processes started from this install are touched.

!macro LUMEN_STOP_DAEMON
  ; Ask the daemon to quit (it ends its web hosts), then make sure.
  IfFileExists "$INSTDIR\lumen.exe" 0 +3
    nsExec::Exec '"$INSTDIR\lumen.exe" quit'
    Pop $0
  nsExec::Exec `powershell -NoProfile -NonInteractive -Command "Start-Sleep -Milliseconds 800; Get-Process lumend, lumen-webhost -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like '$INSTDIR\*' } | Stop-Process -Force"`
  Pop $0
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro LUMEN_STOP_DAEMON
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro LUMEN_STOP_DAEMON
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Updates reinstall over the old version without uninstalling, so this
  ; only runs when Lumen is really removed: drop its sign-in entry.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Lumen"
  ; "Delete the application data" ticked: Lumen's settings and caches. The
  ; caches sit in the install folder (%LOCALAPPDATA%\Lumen), which Tauri's
  ; uninstaller only removes when empty.
  ${If} $DeleteAppDataCheckboxState = 1
    RMDir /r "$APPDATA\Lumen"
    RMDir /r "$LOCALAPPDATA\Lumen\cache"
    RMDir /r "$LOCALAPPDATA\Lumen\webview"
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
