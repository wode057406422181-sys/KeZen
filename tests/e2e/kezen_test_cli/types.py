from dataclasses import dataclass, field

@dataclass
class ToolCall:
    tool_use_id: str
    name: str
    input_json: str

@dataclass
class ToolResult:
    tool_use_id: str
    output: str
    is_error: bool

@dataclass
class PermissionRequestInfo:
    request_id: str
    tool: str
    description: str
    risk_level: int

@dataclass
class TurnResult:
    """Structured result of one message turn (send_message -> Done)."""
    text: str = ""
    thinking: str = ""
    tool_calls: list[ToolCall] = field(default_factory=list)
    tool_results: list[ToolResult] = field(default_factory=list)
    permission_requests: list[PermissionRequestInfo] = field(default_factory=list)
    is_error: bool = False
    error_message: str = ""
    warnings: list[str] = field(default_factory=list)
