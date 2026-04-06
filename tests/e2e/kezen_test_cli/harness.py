import os
import re
import socket
import subprocess
import time
from pathlib import Path

class KezenTestHarness:
    """Manages kezen + mock-llm-server lifecycle via Docker Compose."""
    
    def __init__(
        self,
        fixture_file: str = "smoke.yaml",
        provider: str = "anthropic",
        *,
        compose_project: str | None = None,
    ):
        self.fixture_file = fixture_file
        self.provider = provider
        self._compose_dir = Path(__file__).resolve().parents[1]  # tests/e2e/
        self._compose_project = compose_project or f"kezen-e2e-{provider}"
        self.grpc_host_port: int | None = None
    
    def _compose_env(self) -> dict[str, str]:
        """Environment variables for docker compose commands."""
        return {
            **os.environ,
            "FIXTURE_FILE": self.fixture_file,
            "KEZEN_PROVIDER": self.provider,
            "COMPOSE_DOCKER_CLI_BUILD": "1",
            "DOCKER_BUILDKIT": "1",
        }
    
    def _run_compose(self, *args: str, check: bool = True) -> subprocess.CompletedProcess:
        cmd = [
            "docker", "compose",
            "-p", self._compose_project,
            "-f", str(self._compose_dir / "docker-compose.yml"),
            *args,
        ]
        return subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            check=check,
            env=self._compose_env(),
        )
    
    def build(self):
        """Build container images (cached unless source changed)."""
        self._run_compose("build")
    
    async def start(self, build: bool = True):
        """Build (if needed), start services, discover gRPC port."""
        if build:
            self.build()
        
        self._run_compose("up", "-d", "--wait")
        
        result = self._run_compose("port", "kezen", "50051")
        match = re.search(r":(\d+)", result.stdout.strip())
        if not match:
            raise RuntimeError(
                f"Failed to discover gRPC port. "
                f"stdout={result.stdout!r}, stderr={result.stderr!r}"
            )
        self.grpc_host_port = int(match.group(1))
        self._wait_for_port(self.grpc_host_port, timeout=30.0)
    
    @property
    def grpc_addr(self) -> str:
        assert self.grpc_host_port, "Harness not started"
        return f"127.0.0.1:{self.grpc_host_port}"
    
    async def stop(self):
        self._run_compose("down", "--volumes", "--remove-orphans", check=False)
    
    def logs(self, service: str = "") -> str:
        args = ["logs"]
        if service:
            args.append(service)
        result = self._run_compose(*args, check=False)
        return result.stdout + result.stderr
    
    @staticmethod
    def _wait_for_port(port: int, timeout: float = 15.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                    return
            except (ConnectionRefusedError, OSError):
                time.sleep(0.2)
        raise TimeoutError(f"Port {port} did not become ready in {timeout}s")
