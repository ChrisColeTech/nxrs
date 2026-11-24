@echo off
py -B "%~dp0wonder_tools.py" engine %*
exit /b %errorlevel%
