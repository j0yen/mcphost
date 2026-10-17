"""AC3: arguments are validated locally, before any HTTP request."""

from typing import Any

import pytest
from pydantic import ValidationError

from fake_host import FakeHost, ok
from mcphost_sdk import Client


def test_bad_kind_raises_validation_error_before_any_request() -> None:
    host = FakeHost()
    c = Client("http://host.test", on_skew="ignore", transport=host.transport())
    with pytest.raises(ValidationError) as info:
        c.host.tool.publish(name="x", kind="nope", spec={})  # type: ignore[arg-type]
    assert "kind" in str(info.value)
    assert host.requests == [], "a request went out for an invalid argument"


def test_valid_call_sends_only_the_set_arguments_with_the_key() -> None:
    seen: dict[str, Any] = {}

    def on_call(name: str, args: dict[str, Any]) -> dict[str, Any]:
        seen.update(name=name, args=args)
        return ok({"name": "x", "kind": "echo"})

    host = FakeHost(on_call=on_call)
    c = Client("http://host.test", key="sekrit-key", on_skew="ignore", transport=host.transport())
    c.host.tool.publish(name="x", kind="echo", spec={"schema": {}, "null_inside": None})
    assert seen["name"] == "host.tool.publish"
    assert seen["args"] == {"name": "x", "kind": "echo", "spec": {"schema": {}, "null_inside": None}}
    call = host.requests[-1]
    assert call.headers["authorization"] == "Bearer sekrit-key"
    assert call.url.path == "/mcp"
    assert "sekrit-key" not in repr(c)
