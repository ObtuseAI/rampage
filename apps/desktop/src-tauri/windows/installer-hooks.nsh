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
  Delete "$SMPROGRAMS\Rampage Shell.lnk"
!macroend
