@echo off
py -B "%~dp0wonder_tools.py" xlate %*
exit /b %errorlevel%
