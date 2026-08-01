; Rampage promises a discoverable desktop launcher for every normal and silent NSIS install.
; Tauri's default interactive flow makes this optional on the finish page, so the post-install
; hook deliberately recreates the product shortcut after payload and registry installation.
!macro NSIS_HOOK_POSTINSTALL
  Delete "$DESKTOP\${PRODUCTNAME}.lnk"
  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
!macroend
