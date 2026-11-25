; Hooks del instalador NSIS de Tauri (bundle.windows.nsis.installerHooks).
;
; El instalador es por equipo y corre elevado, así que la CLI puede escribir en HKLM.
; - Al instalar: la carpeta de FrameRush entra en el PATH del sistema (`framerush system-path`).
;   Lo hace la CLI y no NSIS, porque las cadenas de NSIS tienen un tope de 1024 caracteres y
;   podrían truncar un PATH largo.
; - Al desinstalar (no al actualizar, `$UpdateMode` del template de Tauri): se deshace todo
;   como «Restaurar» (`framerush revert`) y se quita la carpeta del PATH, antes de que se borren
;   los archivos.

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog '"$INSTDIR\framerush.exe" system-path'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    nsExec::ExecToLog '"$INSTDIR\framerush.exe" revert'
    Pop $0
    nsExec::ExecToLog '"$INSTDIR\framerush.exe" system-path --remove'
    Pop $0
  ${EndIf}
!macroend
