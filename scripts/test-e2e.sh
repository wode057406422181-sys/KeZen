#!/bin/bash
set -e

KEZEN_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "🚀 Starting KeZen Automated E2E Testing Suite..."

cd "${KEZEN_ROOT}/tests/e2e"

# Ensure Python Virtual Environment is ready
if [ ! -d ".venv" ]; then
    echo "📦 Virtual environment (.venv) not found. Initializing..."
    python3 -m venv .venv
    .venv/bin/pip install -e .
fi

# Make sure Docker compose stops properly on exit if it ever runs directly
echo "🧪 Invoking Pytest..."
.venv/bin/pytest -v

echo "✅ All tests completed smoothly!"
