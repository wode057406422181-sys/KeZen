import pytest

@pytest.mark.timeout(30)
async def test_smoke_text_response(cli, harness):
    """Verify full pipeline: Python -> gRPC -> kezen -> mock-llm-server."""
    result = await cli.send_message("Hello, kezen!")
    
    assert not result.is_error, (
        f"[{harness.provider}] Unexpected error: {result.error_message}"
    )
    assert "Hello from mock" in result.text
    assert "E2E pipeline is working" in result.text
    assert len(result.tool_calls) == 0

@pytest.mark.timeout(30)
async def test_smoke_server_hello(cli):
    """Verify gRPC handshake completed with ServerHello."""
    assert cli._server_hello is not None
    assert cli._server_hello.protocol_version == 1
    assert len(cli._server_hello.server_version) > 0
