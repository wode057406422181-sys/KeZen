import pytest

@pytest.mark.timeout(120)
async def test_cost_command(make_harness, make_cli):
    h = await make_harness("manual/smoke.yaml")
    cli = await make_cli(h)
    await cli.send_message("Hello")
    result = await cli.send_slash_command("/cost")
    assert len(result.slash_command_results) == 1
    assert "Tokens:" in result.slash_command_results[0].output

@pytest.mark.timeout(120)
async def test_clear_command(make_harness, make_cli):
    h = await make_harness("manual/smoke.yaml")
    cli = await make_cli(h)
    await cli.send_message("Hello")
    result = await cli.send_slash_command("/clear")
    assert len(result.slash_command_results) == 1
    assert "cleared" in result.slash_command_results[0].output.lower()

@pytest.mark.timeout(120)
async def test_context_command(make_harness, make_cli):
    h = await make_harness("manual/smoke.yaml")
    cli = await make_cli(h)
    await cli.send_message("Hello")
    result = await cli.send_slash_command("/context")
    assert len(result.slash_command_results) == 1
    assert "Context Budget" in result.slash_command_results[0].output

@pytest.mark.timeout(300)
async def test_resume_restore_shows_history(make_harness, make_cli):
    """After /resume <id>, the client should receive a SessionRestored event
    containing the full conversation history from the restored session."""
    h = await make_harness("manual/smoke.yaml")
    cli = await make_cli(h)

    # Step 1: Create a conversation to generate a persisted session
    await cli.send_message("Hello")

    # Step 2: List sessions to get the session ID
    list_result = await cli.send_slash_command("/resume")
    assert len(list_result.slash_command_results) == 1
    output = list_result.slash_command_results[0].output
    assert "ID:" in output, f"Expected session listing, got: {output}"

    # Extract the session ID from the output
    # Format: "- ID: <uuid> (Model: ..., Msgs: ...)"
    import re
    match = re.search(r"ID:\s+(\S+)\s+\(", output)
    assert match, f"Could not extract session ID from: {output}"
    session_id = match.group(1)

    # Step 3: Resume the session — should receive SessionRestored event
    restore_result = await cli.send_slash_command(f"/resume {session_id}")
    assert len(restore_result.slash_command_results) == 1
    assert "Resumed session" in restore_result.slash_command_results[0].output

    # Step 4: Verify restored messages were received
    assert len(restore_result.restored_messages) > 0, \
        "Expected SessionRestored event with conversation history"
    # Should contain at least a user message ("Hello") and an assistant response
    roles = [m.role for m in restore_result.restored_messages]
    assert "user" in roles, f"Expected user message in restored history, got roles: {roles}"

