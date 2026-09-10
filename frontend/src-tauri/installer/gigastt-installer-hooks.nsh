; Path-scoped installer readiness and transactional payload deployment.
; This file is included before Tauri's template constants; keep references to
; ${MAINBINARYNAME} inside macros so NSIS expands them only at insertion sites.
; Capture __FILEDIR__ now: inside a macro it resolves later at the generated
; installer.nsi invocation directory instead of this source directory.
!define CONVERSATIONALY_INSTALLER_HOOK_DIR "${__FILEDIR__}"

!macro CONVERSATIONALY_RUN_HELPER MODE PAYLOAD UNIQUE_ID
  InitPluginsDir
  File /oname=$PLUGINSDIR\gigastt-installer-preflight.ps1 "${CONVERSATIONALY_INSTALLER_HOOK_DIR}\gigastt-installer-preflight.ps1"

  conversationaly_${UNIQUE_ID}_retry:
    ; NSIS is a 32-bit process. On 64-bit Windows, Sysnative bypasses WOW64
    ; redirection so Get-Process.Path can inspect the 64-bit installed app.
    StrCpy $7 "$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe"
    IfFileExists "$7" +2 0
      StrCpy $7 "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"
    nsExec::ExecToStack '"$7" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\gigastt-installer-preflight.ps1" -Mode ${MODE} -InstallDir "$INSTDIR" -MainBinaryName "${MAINBINARYNAME}.exe" ${PAYLOAD}'
    Pop $8
    Pop $9

    ; nsExec also returns words such as "error" when the child cannot launch.
    ; Never feed a nonnumeric value to SetErrorLevel, where it could become 0.
    StrCmp $8 "0" conversationaly_${UNIQUE_ID}_code_valid
    StrCmp $8 "10" conversationaly_${UNIQUE_ID}_code_valid
    StrCmp $8 "11" conversationaly_${UNIQUE_ID}_code_valid
    StrCmp $8 "12" conversationaly_${UNIQUE_ID}_code_valid
    StrCmp $8 "13" conversationaly_${UNIQUE_ID}_code_valid
    StrCpy $8 13
    conversationaly_${UNIQUE_ID}_code_valid:

    ${If} $9 == ""
      StrCpy $9 "Installer preflight could not start. Close Conversationaly from the tray, then Retry."
    ${EndIf}

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
