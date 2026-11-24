@echo off
py -B "%~dp0wonder_tools.py" spirv %*
exit /b %errorlevel%
