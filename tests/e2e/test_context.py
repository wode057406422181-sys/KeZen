import os
from pathlib import Path
import pytest

# Determine the absolute path to the fixtures directory
FIXTURES_DIR = Path(__file__).parent.parent.parent / "mock-llm-server" / "fixtures"

def fixture_exists(rel_path: str) -> bool:
    """Check if a fixture file exists."""
    return (FIXTURES_DIR / rel_path).exists()

# 2.1 Manual /compact
@pytest.mark.timeout(120)
@pytest.mark.skipif(not fixture_exists("manual/compact.yaml"), reason="Fixture manual/compact.yaml not found")
async def test_manual_compact(make_harness, make_cli):
    h = await make_harness("manual/compact.yaml")
    cli = await make_cli(h)
    await cli.send_message("Topic 1")
    await cli.send_message("Topic 2")
    result = await cli.send_slash_command("/compact")
    assert any("Compacting" in p for p in result.compact_progress)
    assert any("compacted" in p.lower() or "failed" in p.lower() for p in result.compact_progress)

# 2.2 Cache tokens (Anthropic only, manual fixture)
@pytest.mark.timeout(120)
@pytest.mark.skipif(not fixture_exists("manual/anthropic_cache.yaml"), reason="Fixture manual/anthropic_cache.yaml not found")
async def test_cache_usage_reported(make_harness, make_cli):
    h = await make_harness("manual/anthropic_cache.yaml", provider="anthropic")
    cli = await make_cli(h)
    result = await cli.send_message("Testing cache")
    assert len(result.cost_updates) >= 1
    has_cache = any(
        u.cache_creation_input_tokens > 0 or u.cache_read_input_tokens > 0
        for u in result.cost_updates
    )
    assert has_cache
