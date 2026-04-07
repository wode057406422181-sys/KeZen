import os
import requests
import json

base_url = os.environ.get('OPENAI_BASE_URL', "https://dashscope.aliyuncs.com/compatible-mode/v1")
api_key = os.environ.get('OPENAI_API_KEY')
if not api_key:
    print("API KEY missing")
    exit(1)

headers = {"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"}
body = {
    "model": "qwen3.5-flash",
    "messages": [{"role": "user", "content": "Please search the web for the latest news on mars."}],
    "stream": True,
    "enable_search": True
}

resp = requests.post(f"{base_url}/chat/completions", headers=headers, json=body, stream=True)
for line in resp.iter_lines():
    if line:
        print(line.decode('utf-8'))

