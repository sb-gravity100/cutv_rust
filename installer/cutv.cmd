@echo off
rem `cutv <video>` from any terminal (installed next to cutv_rust.exe, added to PATH by the installer)
start "" "%~dp0cutv_rust.exe" %*
