# KeZen E2E Testing Framework

This directory contains the End-to-End (E2E) testing framework for KeZen. 

The framework is designed to test the KeZen engine comprehensively, exactly as it runs in production, while maintaining 100% determinism. It achieves this by stripping out live LLM API calls and replacing them with a deterministic `MockLlmServer` that faithfully replays pre-recorded or manually crafted conversation fixtures.

## Architecture

The framework consists of three massive pillars:

1. **Mock LLM Server** (`mock-llm-server/`)
   A Rust-based HTTP server that mocks the standard OpenAI and Anthropic API endpoints. It reads from YAML fixture files and serves precise stream chunks or JSON responses requested by the KeZen engine.
   
2. **Kezen Test Harness** (`tests/e2e/kezen_test_cli/harness.py`)
   A Python lifecycle manager that utilizes `docker-compose.yml` to spin up isolated container instances of both the Mock Server and the KeZen Engine. It dynamically injects configuration so that KeZen naturally points its API endpoints to the Mock Server.

3. **Kezen Test CLI** (`tests/e2e/kezen_test_cli/client.py`)
   A Python client that speaks gRPC directly to KeZen's `serve-grpc` endpoint. It simulates user input, collects a stream of events (Tool calls, Cost updates, Thoughts, text), and allows `pytest` functions to assert complex system state.

## Fixture Strategy

We utilize a hybrid fixture strategy for maximum coverage and maintainability. All fixtures reside in `mock-llm-server/fixtures/`.

* **Recorded Fixtures** (`fixtures/recorded/`)
  The preferred method for most E2E tests. Instead of manually constructing massive LLM response payloads, we record real traffic between KeZen and Live APIs (like OpenAI or DashScope) to generate YAML fixtures. This ensures our tests reflect actual LLM behavior.
* **Manual Fixtures** (`fixtures/manual/`)
  Hand-crafted minimal YAML files used for testing exact edge cases that are difficult to prompt an LLM to reliably produce (e.g., specific tool failures, context auto-compaction boundaries, skill error injection).

## Recording New Scenarios

To record a new LLM conversation scenario for E2E replays:

1. Add your prompt scenario to the `scenarios` list in `tests/e2e/record_fixtures.py`.
2. Ensure you have the appropriate `OPENAI_API_KEY` exported in your shell.
3. Run the recording script:
   ```bash
   .venv/bin/python record_fixtures.py
   ```
4. The script will orchestrate a live KeZen session using the `recording` pass-through proxy mode, and dump the resultant LLM HTTP transactions into `mock-llm-server/fixtures/recorded/`.
5. Write your new assertions in `test_recorded_scenarios.py` using this fixture.

## Running Tests

To run the entire E2E suite locally:

```bash
# From the root directory of KeZen
sh ./scripts/test-e2e.sh
```

Pytest is configured natively via `pyproject.toml`. It will collect all tests starting with `test_`, utilize `pytest-asyncio` for the coroutine harnesses, and enforce timeout thresholds on the Docker Compose build pipeline.
