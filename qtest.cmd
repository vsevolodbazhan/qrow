@echo off
setlocal
set "UV_NO_DEV=1"
cd /d "%~dp0"
uv run --locked --no-dev --quiet python scripts/qtest %*
exit /b %errorlevel%
