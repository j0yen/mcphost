"""R7 / AC8 (Live): the SDK built from the deployed tag against the real host.

    MCPHOST_LIVE=1 MCPHOST_URL=https://mcphost.dev uv run --directory sdk/python pytest tests/test_live_smoke.py -q
"""

import os

import pytest

from mcphost_sdk import Client
from mcphost_sdk.models import HostQuickstartResult

pytestmark = pytest.mark.skipif(os.environ.get("MCPHOST_LIVE") != "1", reason="set MCPHOST_LIVE=1 to run against a real host")


def test_quickstart_without_a_key_is_typed_and_the_contract_matches() -> None:
    url = os.environ.get("MCPHOST_URL", "https://mcphost.dev")
    with Client(url, key="", on_skew="ignore") as c:
        r = c.host.quickstart()
        assert isinstance(r, HostQuickstartResult)
        assert r.limits is not None and r.limits.plan is not None
        assert c.contract_skew() is None
