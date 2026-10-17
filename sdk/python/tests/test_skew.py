"""AC5: the client notices when the host's contract is not the one it was rendered from."""

import warnings

import pytest

import mcphost_sdk
from fake_host import FakeHost
from mcphost_sdk import Client, ContractSkew, ContractSkewWarning, Skew, _runtime

HOST_SHA = "b71e" + "0" * 60
HOST_VERSION = f"1.{HOST_SHA[:12]}"


@pytest.fixture(autouse=True)
def fresh_process() -> None:
    _runtime._reset_skew_warned()


def test_skew_warns_exactly_once_naming_both_versions() -> None:
    host = FakeHost(etag=f'"{HOST_SHA}"')
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        Client("http://host.test", transport=host.transport())
        Client("http://host.test", transport=host.transport())
    skew = [w for w in caught if issubclass(w.category, ContractSkewWarning)]
    assert len(skew) == 1, "the warning is emitted once per process"
    text = str(skew[0].message)
    assert mcphost_sdk.CONTRACT_VERSION in text and HOST_VERSION in text


def test_raise_mode_raises_contract_skew() -> None:
    host = FakeHost(etag=f'"{HOST_SHA}"')
    with pytest.raises(ContractSkew) as info:
        Client("http://host.test", on_skew="raise", transport=host.transport())
    assert info.value.local == mcphost_sdk.CONTRACT_VERSION
    assert info.value.host == HOST_VERSION


def test_matching_host_is_silent_and_skew_is_none() -> None:
    host = FakeHost()
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        c = Client("http://host.test", on_skew="raise", transport=host.transport())
    assert c.contract_skew() is None
    assert [r.method for r in host.requests] == ["HEAD", "HEAD"]


def test_contract_skew_reports_both_versions() -> None:
    c = Client("http://host.test", on_skew="ignore", transport=FakeHost(etag=f'W/"{HOST_SHA}"').transport())
    assert c.contract_skew() == Skew(local=mcphost_sdk.CONTRACT_VERSION, host=HOST_VERSION)


def test_ignore_mode_makes_no_request() -> None:
    host = FakeHost(etag=f'"{HOST_SHA}"')
    Client("http://host.test", on_skew="ignore", transport=host.transport())
    assert host.requests == []


def test_unknown_on_skew_value_is_refused() -> None:
    with pytest.raises(ValueError):
        Client("http://host.test", on_skew="shout")
