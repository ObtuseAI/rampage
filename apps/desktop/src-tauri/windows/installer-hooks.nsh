; A tray-resident desktop can leave its controller, worker, relay, and intelligence sidecars
; running after the visible window closes. Windows then keeps their executable files locked, and
; NSIS can otherwise report a successful upgrade while leaving old sidecars on disk. Stop only
; Rampage's exact shipped image names before any payload replacement or removal.
!macro RAMPAGE_STOP_RUNNING_PROCESSES
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /T /IM "${MAINBINARYNAME}.exe"'
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /T /IM "rampage-controller.exe"'
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /T /IM "rampage-agent.exe"'
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /T /IM "rampage-relay.exe"'
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /T /IM "rampage-intelligence.exe"'
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /T /IM "rampage.exe"'
  Sleep 1000
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro RAMPAGE_STOP_RUNNING_PROCESSES
!macroend

; Rampage promises a discoverable desktop launcher for every normal and silent NSIS install.
; Tauri's default interactive flow makes this optional on the finish page, so the post-install
; hook deliberately recreates the product shortcut after payload and registry installation.
!macro NSIS_HOOK_POSTINSTALL
  Delete "$DESKTOP\${PRODUCTNAME}.lnk"
  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
  ; Give terminal users a zero-configuration shell without permanently mutating the user's PATH.
  CreateShortcut "$SMPROGRAMS\Rampage Shell.lnk" "$SYSDIR\cmd.exe" '/K "set PATH=$INSTDIR;%PATH%&&cd /d %USERPROFILE%"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro RAMPAGE_STOP_RUNNING_PROCESSES
  Delete "$SMPROGRAMS\Rampage Shell.lnk"
!macroend
