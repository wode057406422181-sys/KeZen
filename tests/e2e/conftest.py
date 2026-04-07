import pytest
import pytest_asyncio
from kezen_test_cli.harness import KezenTestHarness
from kezen_test_cli.client import KezenTestCli

# ── Factory Fixtures ────────────────────────────────────────────

@pytest_asyncio.fixture
async def make_harness():
    """Factory: each test specifies its own fixture file and parameters.

    Usage::

        async def test_something(make_harness):
            h = await make_harness("my_fixture.yaml")
            ...
    """
    instances: list[KezenTestHarness] = []

    async def _factory(
        fixture_file: str,
        provider: str = "anthropic",
        auto_approve: bool = True,
        extra_env: dict[str, str] | None = None,
    ) -> KezenTestHarness:
        import uuid
        project_name = f"kezen-test-{provider}-{uuid.uuid4().hex[:6]}"
        h = KezenTestHarness(
            fixture_file=fixture_file,
            provider=provider,
            auto_approve=auto_approve,
            compose_project=project_name,
        )
        await h.start(extra_env=extra_env)
        instances.append(h)
        return h

    yield _factory

    for h in instances:
        await h.stop()


@pytest_asyncio.fixture
async def make_cli():
    """Factory: create a KezenTestCli connected to a running harness.

    Usage::

        async def test_something(make_harness, make_cli):
            h = await make_harness("manual/smoke.yaml")
            cli = await make_cli(h)
            ...
    """
    clients: list[KezenTestCli] = []

    async def _factory(harness: KezenTestHarness) -> KezenTestCli:
        client = KezenTestCli(harness.grpc_addr)
        await client.connect()
        clients.append(client)
        return client

    yield _factory

    for c in clients:
        await c.close()


# ── Backward-compatible fixtures for test_smoke.py ──────────────
# These wrap the factory fixtures to keep the existing session-scoped
# smoke tests working without modification.

@pytest_asyncio.fixture(scope="session", params=["anthropic", "openai"])
async def harness(request):
    """Session-scoped harness: one per provider (anthropic, openai).

    Kept for backward compatibility with test_smoke.py.
    """
    provider = request.param
    h = KezenTestHarness(fixture_file="manual/smoke.yaml", provider=provider)
    await h.start()
    yield h
    await h.stop()


@pytest_asyncio.fixture
async def cli(harness: KezenTestHarness) -> KezenTestCli:
    """Per-test: fresh gRPC connection to the running kezen container.

    Kept for backward compatibility with test_smoke.py.
    """
    client = KezenTestCli(harness.grpc_addr)
    await client.connect()
    yield client
    await client.close()
