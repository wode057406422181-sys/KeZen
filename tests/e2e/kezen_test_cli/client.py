import asyncio
import grpc
from .generated import kezen_pb2, kezen_pb2_grpc
from .types import TurnResult, ToolCall, ToolResult, PermissionRequestInfo, TokenUsage, SlashCommandResult, RestoredMessage, HistoryBlock

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
    
    async def send_message(
        self,
        content: str,
        timeout: float = 30.0,
        auto_approve_permissions: bool = False,
    ) -> TurnResult:
        """Send a user message and collect all events until Done.

        Args:
            content: The user message text.
            timeout: Maximum seconds to wait for the turn to complete.
            auto_approve_permissions: If True, automatically send AllowOnce
                for every PermissionRequest received during this turn.
        """
        msg = kezen_pb2.ClientMessage(
            send_message=kezen_pb2.SendMessage(content=content)
        )
        await self._outgoing.put(msg)
        
        result = TurnResult()
        
        async def collect():
            async for server_msg in self._stream:
                event = server_msg.WhichOneof("event")
                print(f"[DEBUG] CLI received event: {event}")
                match event:
                    case "text_delta":
                        result.text += server_msg.text_delta.text
                    case "thinking_delta":
                        result.thinking += server_msg.thinking_delta.text
                    case "tool_use_start":
                        t = server_msg.tool_use_start
                        print(f"[DEBUG] CLI received tool_use_start: {t.name}")
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
                        if auto_approve_permissions:
                            await self.respond_permission(p.request_id, allow=True)
                    case "cost_update":
                        u = server_msg.cost_update.usage
                        result.cost_updates.append(TokenUsage(
                            input_tokens=u.input_tokens,
                            output_tokens=u.output_tokens,
                            cache_creation_input_tokens=u.cache_creation_input_tokens,
                            cache_read_input_tokens=u.cache_read_input_tokens,
                        ))
                    case "slash_command_result":
                        s = server_msg.slash_command_result
                        result.slash_command_results.append(SlashCommandResult(
                            command=s.command,
                            output=s.output,
                        ))
                        return
                    case "compact_progress":
                        msg = server_msg.compact_progress.message
                        result.compact_progress.append(msg)
                        if "compacted" in msg.lower() or "failed" in msg.lower():
                            return
                    case "skill_loaded":
                        result.skills_loaded.append(
                            server_msg.skill_loaded.name
                        )
                    case "error":
                        result.is_error = True
                        result.error_message = server_msg.error.message
                    case "warning":
                        result.warnings.append(server_msg.warning.message)
                    case "done":
                        return
                    case "session_restored":
                        import json
                        sr = server_msg.session_restored
                        try:
                            msgs = json.loads(sr.messages_json)
                            for m in msgs:
                                blocks = []
                                for cb in m.get("content", []):
                                    cb_type = cb.get("type", "")
                                    blocks.append(HistoryBlock(
                                        block_type=cb_type,
                                        text=cb.get("text", cb.get("thinking", cb.get("content", ""))),
                                        tool_name=cb.get("name", ""),
                                        tool_input_json=json.dumps(cb.get("input", {})) if "input" in cb else "",
                                        tool_use_id=cb.get("id", cb.get("tool_use_id", "")),
                                        is_error=cb.get("is_error", False),
                                    ))
                                result.restored_messages.append(
                                    RestoredMessage(role=m.get("role", ""), blocks=blocks)
                                )
                        except (json.JSONDecodeError, KeyError):
                            pass
                    case _:
                        pass
        
        await asyncio.wait_for(collect(), timeout=timeout)
        return result
    
    async def send_slash_command(
        self,
        command: str,
        timeout: float = 30.0,
        auto_approve_permissions: bool = False,
    ) -> TurnResult:
        """Convenience wrapper: send a slash command (e.g. '/cost', '/compact').

        Slash commands are sent as regular messages — the Engine parses
        the leading '/' and dispatches internally.
        """
        return await self.send_message(
            content=command,
            timeout=timeout,
            auto_approve_permissions=auto_approve_permissions,
        )
    
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
