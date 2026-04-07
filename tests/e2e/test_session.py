import pytest

@pytest.mark.timeout(30)
async def test_cost_command(make_harness, make_cli):
    h = await make_harness("manual/smoke.yaml")
    cli = await make_cli(h)
    await cli.send_message("Hello")
    result = await cli.send_slash_command("/cost")
    assert len(result.slash_command_results) == 1
    assert "Tokens:" in result.slash_command_results[0].output

@pytest.mark.timeout(30)
async def test_clear_command(make_harness, make_cli):
    h = await make_harness("manual/smoke.yaml")
    cli = await make_cli(h)
    await cli.send_message("Hello")
    result = await cli.send_slash_command("/clear")
    assert len(result.slash_command_results) == 1
    assert "cleared" in result.slash_command_results[0].output.lower()

@pytest.mark.timeout(30)
async def test_context_command(make_harness, make_cli):
    h = await make_harness("manual/smoke.yaml")
    cli = await make_cli(h)
    await cli.send_message("Hello")
    result = await cli.send_slash_command("/context")
    assert len(result.slash_command_results) == 1
    assert "Context Budget" in result.slash_command_results[0].output
