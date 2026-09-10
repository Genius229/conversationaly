; Path-scoped installer readiness and transactional payload deployment.
; This file is included before Tauri's template constants; keep references to
; ${MAINBINARYNAME} inside macros so NSIS expands them only at insertion sites.

!macro CONVERSATIONALY_RUN_HELPER MODE PAYLOAD UNIQUE_ID
  InitPluginsDir
  File /oname=$PLUGINSDIR\gigastt-installer-preflight.ps1 "${__FILEDIR__}\gigastt-installer-preflight.ps1"

  conversationaly_${UNIQUE_ID}_retry:
    nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\gigastt-installer-preflight.ps1" -Mode ${MODE} -InstallDir "$INSTDIR" -MainBinaryName "${MAINBINARYNAME}.exe" ${PAYLOAD}'
    Pop $8
    Pop $9

    ${If} $8 != 0
      DetailPrint "$9"
      ${If} ${Silent}
        SetErrorLevel $8
        Quit
      ${ElseIf} $PassiveMode = 1
        SetErrorLevel $8
        Quit
      ${Else}
        MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$9" IDRETRY conversationaly_${UNIQUE_ID}_retry
        SetErrorLevel $8
        Quit
      ${EndIf}
    ${EndIf}
!macroend

!macro CONVERSATIONALY_PREFLIGHT UNIQUE_ID
  !insertmacro CONVERSATIONALY_RUN_HELPER "Preflight" "" "${UNIQUE_ID}"
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro CONVERSATIONALY_PREFLIGHT "preinstall"
  RMDir /r "$PLUGINSDIR\conversationaly-payload"
  CreateDirectory "$PLUGINSDIR\conversationaly-payload"
  SetOutPath "$PLUGINSDIR\conversationaly-payload"
!macroend

!macro CONVERSATIONALY_DEPLOY_STAGED_PAYLOAD
  !insertmacro CONVERSATIONALY_RUN_HELPER "Deploy" '-PayloadDir "$PLUGINSDIR\conversationaly-payload"' "deploy"
  SetOutPath "$INSTDIR"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro CONVERSATIONALY_PREFLIGHT "preuninstall"
!macroend
