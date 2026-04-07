import os
from pathlib import Path
import pytest

# Determine the absolute path to the fixtures directory
FIXTURES_DIR = Path(__file__).parent.parent.parent / "mock-llm-server" / "fixtures"

def fixture_exists(rel_path: str) -> bool:
    """Check if a fixture file exists."""
    return (FIXTURES_DIR / rel_path).exists()

# 1. Bash Tool Round-trip
@pytest.mark.timeout(120)
@pytest.mark.skipif(not fixture_exists("recorded/tool_bash_roundtrip.yaml"), reason="Fixture tool_bash_roundtrip.yaml not found (not recorded yet)")
async def test_recorded_bash_tool_roundtrip(make_harness, make_cli):
    h = await make_harness("recorded/tool_bash_roundtrip.yaml")
    cli = await make_cli(h)
    result = await cli.send_message("Please use the Bash tool, execute command: echo hello")
    
    assert len(result.tool_calls) >= 1
    assert any(t.name == "Bash" for t in result.tool_calls)
    assert any(not r.is_error for r in result.tool_results)


# 2. Read-only Tool
@pytest.mark.timeout(120)
@pytest.mark.skipif(not fixture_exists("recorded/tool_readonly.yaml"), reason="Fixture tool_readonly.yaml not found (not recorded yet)")
async def test_recorded_tool_readonly(make_harness, make_cli):
    h = await make_harness("recorded/tool_readonly.yaml")
    cli = await make_cli(h)
    result = await cli.send_message("Please use the Glob tool to list the root directory /")
    
    # Read-only tools should execute directly without generating permission requests
    # unless auto_approve is turned off globally
    assert len(result.tool_calls) >= 1
    assert len(result.permission_requests) == 0


# 3. Multi-tool Parallel Execution
@pytest.mark.timeout(120)
@pytest.mark.skipif(not fixture_exists("recorded/tool_multi_parallel.yaml"), reason="Fixture tool_multi_parallel.yaml not found (not recorded yet)")
async def test_recorded_tool_multi_parallel(make_harness, make_cli):
    h = await make_harness("recorded/tool_multi_parallel.yaml")
    cli = await make_cli(h)
    result = await cli.send_message("Please use the Bash tool to execute two commands parallelly: echo A and echo B")
    
    # Should trigger multiple tool calls
    assert len(result.tool_calls) >= 2


# 4. Simple Multi-turn Chat
@pytest.mark.timeout(120)
@pytest.mark.skipif(not fixture_exists("recorded/simple_multi_turn_chat.yaml"), reason="Fixture simple_multi_turn_chat.yaml not found (not recorded yet)")
async def test_recorded_simple_multi_turn_chat(make_harness, make_cli):
    h = await make_harness("recorded/simple_multi_turn_chat.yaml")
    cli = await make_cli(h)
    
    # Turn 1
    result1 = await cli.send_message("Hello, my name is Kezen")
    assert not result1.is_error
    
    # Turn 2: the model should recall the context from the previous turn
    result2 = await cli.send_message("What is my name? Please answer.")
    assert not result2.is_error
    
    # The output from the assistant should contain the name
    combined_output = result2.text
    assert "Kezen" in combined_output or "kezen" in combined_output.lower()


