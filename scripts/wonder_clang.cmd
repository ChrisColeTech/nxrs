@echo off
py -B "%~dp0wonder_tools.py" clang %*
exit /b %errorlevel%
