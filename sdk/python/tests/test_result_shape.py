"""AC6: results are parsed forward-compatibly; a missing required field is a named error."""

from typing import Any

import pytest

from fake_host import FakeHost, ok
from mcphost_sdk import Client, ResultShapeError

CHECKOUT = {
    "expires_at": 1,
    "instructions": "pay here",
    "mode": "payment",
    "source": "manual",
    "url": "https://pay.test/x",
}


def client_replying(structured: dict[str, Any]) -> Client:
    host = FakeHost(on_call=lambda _n, _a: ok(structured))
    return Client("http://host.test", on_skew="ignore", transport=host.transport())


def test_missing_required_field_names_the_tool_and_the_field() -> None:
    body = {k: v for k, v in CHECKOUT.items() if k != "url"}
    with pytest.raises(ResultShapeError) as info:
        client_replying(body).billing.checkout()
    assert info.value.tool == "billing.checkout"
    assert info.value.field == "url"
    assert "billing.checkout" in str(info.value) and "url" in str(info.value)


def test_extra_field_is_accepted_and_reachable_via_model_extra() -> None:
    r = client_replying({**CHECKOUT, "added_by_a_newer_host": {"n": 1}}).billing.checkout()
    assert r.url == "https://pay.test/x"
    assert r.model_extra == {"added_by_a_newer_host": {"n": 1}}


def test_wrong_type_is_a_shape_error_too() -> None:
    with pytest.raises(ResultShapeError) as info:
        client_replying({**CHECKOUT, "expires_at": "soon"}).billing.checkout()
    assert info.value.field == "expires_at"


def test_response_without_structured_content_is_a_shape_error() -> None:
    host = FakeHost(on_call=lambda _n, _a: {"result": {"content": []}})
    c = Client("http://host.test", on_skew="ignore", transport=host.transport())
    with pytest.raises(ResultShapeError):
        c.billing.checkout()
