import pytest
import pytest_asyncio
from kezen_test_cli.harness import KezenTestHarness
from kezen_test_cli.client import KezenTestCli

# ── Harness fixtures (session-scoped per provider) ──────────

@pytest_asyncio.fixture(scope="session", params=["anthropic", "openai"])
async def harness(request):
    """Session-scoped harness: one per provider (anthropic, openai)."""
    provider = request.param
    # TODO: This hardcodes smoke.yaml. For provider-specific tests (e.g. Anthropic cache),
    # we will need separate fixture generators (anthropic_harness, openai_harness) instead
    # of a session-scoped generic one.
    h = KezenTestHarness(fixture_file="smoke.yaml", provider=provider)
    await h.start()
    yield h
    await h.stop()

@pytest_asyncio.fixture
async def cli(harness: KezenTestHarness) -> KezenTestCli:
    """Per-test: fresh gRPC connection to the running kezen container."""
    client = KezenTestCli(harness.grpc_addr)
    await client.connect()
    yield client
    await client.close()
