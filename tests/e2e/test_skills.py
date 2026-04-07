import pytest

@pytest.mark.timeout(30)
async def test_skill_load_via_tool(make_harness, make_cli):
    h = await make_harness("manual/skill_load.yaml")
    cli = await make_cli(h)
    result = await cli.send_message("Please use test-skill")
    assert any("test-skill" in s for s in result.skills_loaded)

@pytest.mark.timeout(30)
async def test_skill_not_found(make_harness, make_cli):
    h = await make_harness("manual/skill_not_found.yaml")
    cli = await make_cli(h)
    result = await cli.send_message("Please use a non-existent skill")
    assert any(r.is_error for r in result.tool_results)
