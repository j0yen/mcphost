"""AC4: host error codes become typed exceptions."""

from typing import Any

import pytest

from fake_host import FakeHost, rpc_error
from mcphost_sdk import Client, MCPHostError, errors


def client_for(data: dict[str, Any]) -> Client:
    host = FakeHost(on_call=lambda _n, _a: rpc_error(data, "spec is wrong"))
    return Client("http://host.test", on_skew="ignore", transport=host.transport())


def test_args_invalid_is_a_typed_exception_with_data() -> None:
    c = client_for({"error_code": "args_invalid", "argument": "spec", "remedy": "pass an object"})
    with pytest.raises(errors.ArgsInvalid) as info:
        c.host.tool.publish(name="x", kind="echo", spec={})
    e = info.value
    assert e.code == "args_invalid"
    assert e.data.argument == "spec"
    assert e.remedy == "pass an object"
    assert isinstance(e, MCPHostError)
    assert e.message == "spec is wrong"


def test_unknown_code_raises_the_base_class_with_the_code_preserved() -> None:
    c = client_for({"error_code": "brand_new_code_from_a_newer_host"})
    with pytest.raises(MCPHostError) as info:
        c.host.tool.publish(name="x", kind="echo", spec={})
    assert type(info.value) is MCPHostError
    assert info.value.code == "brand_new_code_from_a_newer_host"


def test_every_registered_code_maps_to_its_own_subclass() -> None:
    assert len(errors.ERRORS_BY_CODE) >= 30
    for code, cls in errors.ERRORS_BY_CODE.items():
        assert issubclass(cls, MCPHostError)
        assert cls("m").code == code
