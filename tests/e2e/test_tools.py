import pytest

# 1.3 FileWrite permission flow (requires auto_approve=False)
@pytest.mark.timeout(120)
async def test_file_write_permission_flow(make_harness, make_cli):
    h = await make_harness("manual/tool_write_permission.yaml", auto_approve=False)
    cli = await make_cli(h)
    result = await cli.send_message("Write to file", auto_approve_permissions=True)
    assert len(result.permission_requests) >= 1



# 1.5 Tool error propagation
@pytest.mark.timeout(120)
async def test_tool_error_propagation(make_harness, make_cli):
    h = await make_harness("manual/tool_error.yaml")
    cli = await make_cli(h)
    result = await cli.send_message("Read a non-existent file")
    assert any(r.is_error for r in result.tool_results)
