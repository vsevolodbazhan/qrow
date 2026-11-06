@echo off
cd /d "%~dp0"
uv run --locked --quiet python scripts/qtest %*
exit /b %errorlevel%
