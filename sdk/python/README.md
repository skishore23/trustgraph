# trustgraph-sdk

The Python client for [`trustgraph serve`](../../docs/serve.md), with
integrations for agent frameworks. Python 3.10 or newer; the core uses only
the standard library.

```sh
pip install ./sdk/python                     # the core
pip install "./sdk/python[langchain]"        # or [openai-agents], [claude-agent-sdk]
```

- Reference: [docs/sdk-python.md](../../docs/sdk-python.md)
- Integrations: [docs/integrations.md](../../docs/integrations.md)

Tests start a real `trustgraph serve` (built with `cargo build -p
trustgraph-cli`, or set `TRUSTGRAPH_BIN`):

```sh
pip install -e "./sdk/python[test]"
pytest sdk/python/tests
```
