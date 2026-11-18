; Tauri NSIS hooks for J.A.R.V.I.S. (per-user install, no administrator rights). UTF-8 with BOM (Unicode NSIS).
;
; Uninstall: Jarvis keeps its data (%APPDATA%\Jarvis: settings, memory, audit log; %LOCALAPPDATA%\Jarvis:
; downloaded models and worker environments) unless the user answers "Yes" to the question below.
; The default answer is "No"; a silent uninstall (/S) never deletes data. The text follows the installer language.

; The desktop shortcut is optional and OFF by default: the checkbox on the finish page starts unchecked.
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED

!macro NSIS_HOOK_PREINSTALL
!macroend

; A silent install (/S) must not put a shortcut on the desktop (Tauri creates one for silent installs).
!macro NSIS_HOOK_POSTINSTALL
  ${If} ${Silent}
    !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ${If} $LANGUAGE = 1049
      MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Удалить данные J.A.R.V.I.S. и скачанные модели?$\r$\n$\r$\nНастройки, память и журнал: $APPDATA\Jarvis$\r$\nМодели и окружения: $LOCALAPPDATA\Jarvis$\r$\n$\r$\nЕсли нажать «Нет», данные останутся (удобно для переустановки)." /SD IDNO IDNO jarvis_keep_data
    ${Else}
      MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Delete J.A.R.V.I.S. data and downloaded models?$\r$\n$\r$\nSettings, memory and log: $APPDATA\Jarvis$\r$\nModels and environments: $LOCALAPPDATA\Jarvis$\r$\n$\r$\nChoose No to keep them (handy when you reinstall)." /SD IDNO IDNO jarvis_keep_data
    ${EndIf}
      RMDir /r "$APPDATA\Jarvis"
      RMDir /r "$LOCALAPPDATA\Jarvis"
    jarvis_keep_data:
  ${EndIf}
!macroend