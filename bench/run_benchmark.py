#!/usr/bin/env python3
"""LLM-as-DOM Benchmark: tests multiple LLM backends on DOM reasoning tasks."""

import json, time, os, re, sys

ZAI_KEY = os.environ.get("Z_AI_API_KEY", "")
ZAI_URL = "https://api.z.ai/api/anthropic/v1/messages"
OLLAMA_URL = os.environ.get("OLLAMA_URL", "http://localhost:11434")

SCENARIOS = [
    {"id": "login_hn", "name": "HN Login", "difficulty": "easy",
     "goal": "login as testuser with password test123", "expected": "type",
     "view": "[0] Input type=text name=\"acct\"\n[1] Input type=password name=\"pw\"\n[2] Button type=submit val=\"login\""},
    {"id": "login_github", "name": "GitHub Login", "difficulty": "medium",
     "goal": "login as test@example.com with password secret123", "expected": "type",
     "view": "[1] Input type=text \"Username or email\" name=\"login\"\n[2] Input type=password \"Password\" name=\"password\"\n[4] Button type=submit val=\"Sign in\""},
    {"id": "todo_add", "name": "TodoMVC Add", "difficulty": "easy",
     "goal": "type 'buy groceries' in the todo input", "expected": "type",
     "view": "[1] Input ph=\"What needs to be done?\"\n[2] Link \"TodoMVC\""},
    {"id": "google_search", "name": "Google Search", "difficulty": "medium",
     "goal": "search for 'rust programming'", "expected": "type",
     "view": "[0] Input type=text \"Search\" name=\"q\"\n[3] Button \"Google Search\""},
]

MODELS = [
    {"name": "qwen3-8b", "type": "ollama", "model": "qwen3:8b"},
    {"name": "qwen2.5-7b", "type": "ollama", "model": "qwen2.5:7b"},
    {"name": "glm-4.5-flash", "type": "zai", "model": "glm-4.5-flash"},
    {"name": "glm-4.5-air", "type": "zai", "model": "glm-4.5-air"},
    {"name": "glm-4.7", "type": "zai", "model": "glm-4.7"},
]

def prompt(sc):
    return f"""You are a browser pilot. Pick the next action.
GOAL: {sc['goal']}
ELEMENTS:
{sc['view']}

Respond ONLY with a single JSON object:
{{"action":"type","element":<id>,"value":"<text>","reasoning":"<why>"}}
or {{"action":"click","element":<id>,"reasoning":"<why>"}}
JSON:"""

def call(m, p):
    import requests
    t0 = time.time()
    try:
        if m["type"] == "ollama":
            r = requests.post(f"{OLLAMA_URL}/api/generate",
                json={"model": m["model"], "prompt": p, "stream": False,
                      "options": {"temperature": 0.1, "num_predict": 2048}}, timeout=90)
            d = r.json()
            text = re.sub(r'<think>.*?</think>', '', d.get("response", ""), flags=re.DOTALL).strip()
            return text, time.time() - t0, d.get("eval_count", 0)
        else:
            r = requests.post(ZAI_URL,
                json={"model": m["model"], "max_tokens": 300,
                      "messages": [{"role": "user", "content": p}]},
                headers={"Content-Type": "application/json",
                         "x-api-key": ZAI_KEY,
                         "anthropic-version": "2023-06-01"}, timeout=90)
            d = r.json()
            if "error" in d:
                return f"API_ERROR: {d['error']}", time.time() - t0, 0
            text = d.get("content", [{}])[0].get("text", "")
            u = d.get("usage", {})
            return text, time.time() - t0, u.get("input_tokens", 0) + u.get("output_tokens", 0)
    except Exception as e:
        return f"ERROR: {e}", time.time() - t0, 0

def parse_json(text):
    text = re.sub(r'```json\s*', '', text)
    text = re.sub(r'```\s*', '', text)
    text = text.strip()
    s = text.find('{')
    if s < 0: return None
    depth = 0
    for i in range(s, len(text)):
        if text[i] == '{': depth += 1
        elif text[i] == '}':
            depth -= 1
            if depth == 0:
                try: return json.loads(text[s:i+1])
                except: return None
    return None

def run():
    print("=" * 74)
    print("  LLM-as-DOM Benchmark Suite")
    print("  Scenarios:", len(SCENARIOS), "| Models:", len(MODELS))
    print("=" * 74)
    
    all_results = []
    for sc in SCENARIOS:
        p = prompt(sc)
        ptok = len(p) // 4
        print(f"\n--- {sc['name']} ({sc['difficulty']}) | ~{ptok} prompt tokens ---")
        
        for m in MODELS:
            text, lat, tok = call(m, p)
            parsed = parse_json(text)
            ok = (parsed is not None
                  and parsed.get("action") == sc["expected"]
                  and parsed.get("element") is not None)
            
            act = f"{parsed['action']}[{parsed.get('element','')}]" if parsed else "NO_JSON"
            val = str(parsed.get("value", ""))[:20] if parsed else ""
            status = "\033[32mPASS\033[0m" if ok else "\033[31mFAIL\033[0m"
            
            if not parsed and text and not text.startswith("ERROR"):
                # Show what we got
                act = f"RAW:{text[:30]}"
            
            print(f"  {m['name']:18s} {status}  {act:24s} {val:22s} {lat:5.1f}s {tok:5d}tok")
            
            all_results.append({
                "scenario": sc["id"], "model": m["name"], "type": m["type"],
                "pass": ok, "action": parsed.get("action") if parsed else None,
                "element": parsed.get("element") if parsed else None,
                "value": parsed.get("value") if parsed else None,
                "latency_s": round(lat, 2), "tokens": tok
            })
    
    # Summary
    print(f"\n{'=' * 74}")
    print("  MODEL RANKING")
    print(f"{'=' * 74}")
    
    stats = {}
    for r in all_results:
        m = r["model"]
        if m not in stats:
            stats[m] = {"p": 0, "t": 0, "lat": [], "tok": [], "type": r["type"]}
        stats[m]["t"] += 1
        if r["pass"]: stats[m]["p"] += 1
        stats[m]["lat"].append(r["latency_s"])
        stats[m]["tok"].append(r["tokens"])
    
    print(f"\n  {'Model':18s} {'Score':8s} {'Avg Lat':8s} {'Avg Tok':8s} {'Cost/call':10s} {'Type':8s}")
    print(f"  {'-' * 64}")
    for m, s in sorted(stats.items(), key=lambda x: (-x[1]["p"], sum(x[1]["lat"]))):
        acc = f"{s['p']}/{s['t']}"
        al = f"{sum(s['lat'])/len(s['lat']):.1f}s"
        at = f"{int(sum(s['tok'])/max(len(s['tok']),1))}"
        cost = "free" if s["type"] == "ollama" else "~$0.001"
        print(f"  {m:18s} {acc:8s} {al:8s} {at:8s} {cost:10s} {s['type']:8s}")
    
    with open("bench/results.json", "w") as f:
        json.dump(all_results, f, indent=2)
    print(f"\n  Saved: bench/results.json")

if __name__ == "__main__":
    run()
