import os
import asyncio
import sys
import shutil
from pathlib import Path

# Add kezen_test_cli and e2e to python path
e2e_dir = Path(__file__).resolve().parent
sys.path.insert(0, str(e2e_dir))

from kezen_test_cli.harness import KezenTestHarness
from kezen_test_cli.client import KezenTestCli
import grpc

import yaml
import argparse

async def record_scenario(name, prompts, provider="anthropic", auto_approve=True, extra_env=None):
    print(f"\n--- Recording {name} ---")
    
    if isinstance(prompts, str):
        prompts = [prompts]
    
    fixtures_dir = e2e_dir.parent.parent / "mock-llm-server" / "fixtures"
    recordings_dir = fixtures_dir / "recordings"
    
    # 1. Clean recordings dir
    if recordings_dir.exists():
        shutil.rmtree(recordings_dir)
    recordings_dir.mkdir(parents=True, exist_ok=True)
    
    # 2. Setup mock env
    if provider == "anthropic":
        anthropic_url = os.environ.get("ANTHROPIC_BASE_URL", "https://api.anthropic.com")
        os.environ["MOCK_CMD"] = f"mock-llm-server proxy --port 3000 --anthropic-url {anthropic_url} --out-dir /fixtures/recordings"
        os.environ["KEZEN_API_KEY"] = os.environ.get("ANTHROPIC_API_KEY")
        os.environ["KEZEN_MODEL"] = os.environ.get("ANTHROPIC_MODEL", "claude-3-7-sonnet-20250219")
    elif provider == "openai":
        openai_url = os.environ.get("OPENAI_BASE_URL", "https://api.openai.com")
        os.environ["MOCK_CMD"] = f"mock-llm-server proxy --port 3000 --openai-url {openai_url} --out-dir /fixtures/recordings"
        os.environ["KEZEN_API_KEY"] = os.environ.get("OPENAI_API_KEY")
        os.environ["KEZEN_MODEL"] = os.environ.get("OPENAI_MODEL", "gpt-4o")
    else:
        print(f"Unsupported provider: {provider}")
        return False
        
    if extra_env:
        for k, v in extra_env.items():
            os.environ[k] = v
    
    # 3. Start harness
    import uuid
    project_name = f"kezen-record-{uuid.uuid4().hex[:6]}"
    # We pass 'record.yaml' (or any name) as fixture file placeholder, proxy mode ignores it anyway.
    h = KezenTestHarness(fixture_file="manual/smoke.yaml", provider=provider, auto_approve=auto_approve, compose_project=project_name)
    await h.start()
    
    # 4. Run client
    cli = KezenTestCli(h.grpc_addr)
    await cli.connect()
    
    print(f"Scenario has {len(prompts)} turn(s).")
    for idx, prompt in enumerate(prompts):
        print(f"[{idx+1}/{len(prompts)}] Sending prompt: {prompt}")
        await cli.send_message(prompt)
        await asyncio.sleep(1) # Extra buffer for proxy to write
    
    await cli.close()
    await h.stop()
    
    # 5. Find generated yaml
    yaml_files = list(recordings_dir.glob("*.yaml"))
    
    if not yaml_files:
        print(f"FAILED: No recording generated for {name}.")
        return False
        
    # Sort files by modification time
    yaml_files.sort(key=os.path.getmtime)
    
    target_path = fixtures_dir / "recorded" / f"{name}.yaml"
    
    # Merge all yaml files into one list
    merged_entries = []
    for f in yaml_files:
        with open(f, "r") as yf:
            parts = yaml.safe_load(yf)
            if parts:
                merged_entries.extend(parts)
                
    with open(target_path, "w") as out:
        yaml.safe_dump(merged_entries, out, allow_unicode=True, sort_keys=False)
        
    print(f"Successfully recorded {name} (merged {len(yaml_files)} entries) -> {target_path.name}")
    
    # Cleanup
    shutil.rmtree(recordings_dir)
    return True

async def main():
    parser = argparse.ArgumentParser(description="Record E2E Fixtures")
    parser.add_argument("--provider", default="anthropic", choices=["anthropic", "openai"], help="The LLM provider to record against")
    args = parser.parse_args()
    
    provider = args.provider
        
    scenarios = [
        ("tool_bash_roundtrip", "Please use the Bash tool, execute command: echo hello", None, None),
        ("tool_multi_parallel", "Please execute the following two commands concurrently (not sequentially!): 1. echo one, 2. echo two", None, None),
        ("simple_multi_turn_chat", ["Hello, my name is Kezen", "What is my name? Please answer."], None, None),
    ]
    
    success_count = 0
    for name, prompt, extra_env, scenario_provider in scenarios:
        active_provider = scenario_provider if scenario_provider else provider
        
        # Check keys based on active provider
        if active_provider == "anthropic" and not os.environ.get("ANTHROPIC_API_KEY"):
            print(f"Skipping {name}: ANTHROPIC_API_KEY required.", file=sys.stderr)
            continue
        elif active_provider == "openai" and not os.environ.get("OPENAI_API_KEY"):
            print(f"Skipping {name}: OPENAI_API_KEY required.", file=sys.stderr)
            continue
            
        success = await record_scenario(name, prompt, provider=active_provider, extra_env=extra_env)
        if success:
            success_count += 1
            
    print(f"\nRecording completed: {success_count}/{len(scenarios)} successful.")
            
if __name__ == "__main__":
    asyncio.run(main())
