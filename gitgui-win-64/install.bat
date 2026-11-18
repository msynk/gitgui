@echo off
setlocal

rem Folder of this script, without the trailing backslash
set "GITGUI_DIR=%~dp0"
if "%GITGUI_DIR:~-1%"=="\" set "GITGUI_DIR=%GITGUI_DIR:~0,-1%"
set "GITGUI_EXE=%GITGUI_DIR%\gitgui.exe"

if not exist "%GITGUI_EXE%" (
    echo gitgui.exe was not found next to this script.
    goto fail
)

echo Adding "%GITGUI_DIR%" to the user PATH...
rem Reads the raw user PATH (keeps %%VAR%% references), appends the folder once,
rem writes it back as REG_EXPAND_SZ and broadcasts the change to running programs.
powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference='Stop'; $d=$env:GITGUI_DIR; $k=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); $p=[string]$k.GetValue('Path','',[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames); $parts=@($p -split ';' | Where-Object { $_ }); if ($parts | Where-Object { $_.TrimEnd('\') -ieq $d }) { Write-Host '  Already in PATH.' } else { $k.SetValue('Path', (($parts + $d) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString); [Environment]::SetEnvironmentVariable('GITGUI_PATH_REFRESH','1','User'); [Environment]::SetEnvironmentVariable('GITGUI_PATH_REFRESH',$null,'User'); Write-Host '  Done.' }; $k.Close()"
if errorlevel 1 goto fail

echo Adding "Open with GitGui" to the Explorer context menu...
rem Same keys the app uses, so "gitgui --uninstall-context-menu" removes them.
call :addmenu "HKCU\Software\Classes\Directory\shell\GitGui" || goto fail
call :addmenu "HKCU\Software\Classes\Directory\Background\shell\GitGui" || goto fail
echo   Done.

echo.
echo GitGui is installed. Open a new terminal and run: gitgui .
echo Right-click a folder (or empty space inside one) in Explorer and pick "Open with GitGui".
echo On Windows 11 the entry is under "Show more options" (Shift+F10).
echo.
pause
exit /b 0

:fail
echo.
echo Installation failed.
pause
exit /b 1

rem Kept outside any ( ) block: a ")" in the install path, such as "Program Files (x86)", would end the block early.
:addmenu
reg add "%~1" /ve /d "Open with GitGui" /f >nul || exit /b 1
reg add "%~1" /v Icon /d "%GITGUI_EXE%" /f >nul || exit /b 1
reg add "%~1\command" /ve /d "\"%GITGUI_EXE%\" \"%%V\"" /f >nul || exit /b 1
exit /b 0
