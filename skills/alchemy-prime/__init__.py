"""Alchemy MCP integration for Prime Agent (installed by Alchemy's
Settings → Agents connect; the layout follows prime-agent's
docs/mcp-integrations.md reference implementation)."""

from rlm import McpIntegration


class Alchemy(McpIntegration):
    # Matches the mcpServers key Alchemy writes to ~/.prime/agent/settings.json;
    # the host-resolved URL from that entry wins over this fallback, so a
    # non-default MCP port keeps working.
    server = "alchemy"
    url = "http://127.0.0.1:41414/mcp"

    async def _resolve_token(self) -> str:
        # Alchemy's MCP server is loopback-only and unauthenticated. The base
        # class refuses to connect without a bearer token, so hand it a
        # placeholder rather than gating a local server on auth.json.
        return "local"


alchemy = Alchemy()

# Forward bare module access (`import alchemy; await alchemy.<tool>(...)`) to
# the instance, but NOT the names the kernel bootstrap probes — forwarding
# `run` would make it treat the module as a callable skill and break tool
# dispatch.
_RESERVED = {"run", "__wrapped__", "__call__"}


def __getattr__(name):
    if name.startswith("_") or name in _RESERVED:
        raise AttributeError(name)
    return getattr(alchemy, name)
