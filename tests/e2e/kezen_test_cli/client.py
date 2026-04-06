import asyncio
import grpc
from .generated import kezen_pb2, kezen_pb2_grpc
from .types import TurnResult, ToolCall, ToolResult, PermissionRequestInfo

class KezenTestCli:
    """gRPC bidirectional stream client for testing KeZen Engine."""
    
    def __init__(self, addr: str):
        self._addr = addr
        self._channel: grpc.aio.Channel | None = None
        self._stub = None
        self._stream = None
        self._outgoing: asyncio.Queue = asyncio.Queue()
        self._server_hello = None
    
    async def connect(self, timeout: float = 10.0):
        self._channel = grpc.aio.insecure_channel(self._addr)
        await asyncio.wait_for(
            self._channel.channel_ready(), timeout=timeout
        )
        self._stub = kezen_pb2_grpc.KezenAgentStub(self._channel)
        self._stream = self._stub.StreamSession(self._request_iterator())
        
        first_msg = await asyncio.wait_for(
            self._stream.__aiter__().__anext__(), timeout=5.0
        )
        assert first_msg.HasField("server_hello"), \
            f"Expected ServerHello, got: {first_msg}"
        self._server_hello = first_msg.server_hello
    
    async def _request_iterator(self):
        yield kezen_pb2.ClientMessage(
            hello=kezen_pb2.Hello(
                protocol_version=1,
                client_name="kezen-test-cli",
            )
        )
        while True:
            msg = await self._outgoing.get()
            if msg is None:
                return
            yield msg
    
    async def send_message(self, content: str, timeout: float = 30.0) -> TurnResult:
        # TODO: This blocks and clears stream until "Done". It is not re-entrant.
        # For multi-turn chats, we may need a global collector task and turn identifiers
        # from the Engine to correctly route events.
        msg = kezen_pb2.ClientMessage(
            send_message=kezen_pb2.SendMessage(content=content)
        )
        await self._outgoing.put(msg)
        
        result = TurnResult()
        
        async def collect():
            async for server_msg in self._stream:
                event = server_msg.WhichOneof("event")
                match event:
                    case "text_delta":
                        result.text += server_msg.text_delta.text
                    case "thinking_delta":
                        result.thinking += server_msg.thinking_delta.text
                    case "tool_use_start":
                        t = server_msg.tool_use_start
                        result.tool_calls.append(ToolCall(
                            tool_use_id=t.tool_use_id,
                            name=t.name,
                            input_json=t.input_json,
                        ))
                    case "tool_result":
                        t = server_msg.tool_result
                        result.tool_results.append(ToolResult(
                            tool_use_id=t.tool_use_id,
                            output=t.output,
                            is_error=t.is_error,
                        ))
                    case "permission_request":
                        p = server_msg.permission_request
                        result.permission_requests.append(PermissionRequestInfo(
                            request_id=p.request_id,
                            tool=p.tool,
                            description=p.description,
                            risk_level=p.risk_level,
                        ))
                    case "error":
                        result.is_error = True
                        result.error_message = server_msg.error.message
                    case "warning":
                        result.warnings.append(server_msg.warning.message)
                    case "done":
                        return
                    case _:
                        pass
        
        await asyncio.wait_for(collect(), timeout=timeout)
        return result
    
    async def respond_permission(self, request_id: str, allow: bool):
        if allow:
            pr = kezen_pb2.PermissionResponse(
                request_id=request_id,
                allow_once=kezen_pb2.AllowOnce(),
            )
        else:
            pr = kezen_pb2.PermissionResponse(
                request_id=request_id,
                deny=kezen_pb2.Deny(),
            )
        await self._outgoing.put(
            kezen_pb2.ClientMessage(permission_response=pr)
        )
    
    async def cancel(self):
        await self._outgoing.put(
            kezen_pb2.ClientMessage(cancel=kezen_pb2.Cancel())
        )
    
    async def close(self):
        await self._outgoing.put(None)
        if self._channel:
            await self._channel.close()
