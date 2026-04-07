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
class TokenUsage:
    input_tokens: int = 0
    output_tokens: int = 0
    cache_creation_input_tokens: int = 0
    cache_read_input_tokens: int = 0

@dataclass
class SlashCommandResult:
    command: str = ""
    output: str = ""

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
    # P-1: New fields for extended event capture
    cost_updates: list[TokenUsage] = field(default_factory=list)
    slash_command_results: list[SlashCommandResult] = field(default_factory=list)
    compact_progress: list[str] = field(default_factory=list)
    skills_loaded: list[str] = field(default_factory=list)
