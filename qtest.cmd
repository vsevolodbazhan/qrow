@echo off
cd /d "%~dp0"
uv run --locked --no-dev --quiet python scripts/qtest %*
exit /b %errorlevel%
