!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Parley Health Supervisor" '$\"$INSTDIR\health\parley-health-supervisor.exe$\"'
  ExecWait '$\"$INSTDIR\health\parley-health-supervisor.exe$\" --refresh-installation'
  Exec '$\"$INSTDIR\health\parley-health-supervisor.exe$\"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '$\"$INSTDIR\parley-viewer.exe$\" --exit'
  ExecWait '$\"$INSTDIR\health\parley-health-supervisor.exe$\" --remove-hooks'
  ExecWait '$\"$INSTDIR\health\parley-health-supervisor.exe$\" --shutdown'
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Parley Health Supervisor"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Parley Conversation Viewer"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  RMDir /r "$APPDATA\com.ickleslimer.parley-viewer"
!macroend

