from __future__ import annotations

from typing import Any, Mapping, Optional, cast

from pydantic import BaseModel, ConfigDict


class ErrorData(BaseModel):
    """The `data` object of a host error; every key the host adds is reachable."""

    model_config = ConfigDict(extra="allow")

    error_code: Optional[str] = None
    argument: Optional[str] = None
    field: Optional[str] = None
    expected: Any = None
    remedy: Optional[str] = None
    docs: Optional[str] = None
    help_url: Optional[str] = None
    request_id: Optional[str] = None


class MCPHostError(Exception):
    """Base class of every error the host answers with; `code` is the wire code."""

    def __init__(self, code: str, message: str, data: Optional[ErrorData] = None) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message
        self.data = data if data is not None else ErrorData()

    @property
    def remedy(self) -> Optional[str]:
        return self.data.remedy


class ContractSkewWarning(UserWarning):
    """The host's contract is not the one this SDK was rendered from."""


class ContractSkew(Exception):
    """Raised by `Client(on_skew="raise")` when the host's contract differs."""

    def __init__(self, local: str, host: str) -> None:
        super().__init__(f"contract skew: sdk rendered from {local}, host serves {host}")
        self.local = local
        self.host = host


class ResultShapeError(Exception):
    """A response did not have the shape the contract promises for `tool`."""

    def __init__(self, tool: str, field: str, detail: str) -> None:
        super().__init__(f"{tool}: result field {field!r}: {detail}")
        self.tool = tool
        self.field = field
        self.detail = detail


def error_from_wire(error: Mapping[str, Any]) -> MCPHostError:
    """Build the typed exception for a JSON-RPC `error` member (or a tool error body)."""
    raw = error.get("data")
    data = ErrorData.model_validate(raw if isinstance(raw, dict) else {})
    code = data.error_code
    if code is None and isinstance(raw, dict) and isinstance(raw.get("code"), str):
        code = raw["code"]
    if code is None:
        code = error["code"] if isinstance(error.get("code"), str) else "unknown"
    message = str(error.get("message", ""))
    cls = ERRORS_BY_CODE.get(code, MCPHostError)
    if cls is MCPHostError:
        return MCPHostError(code, message, data)
    # every generated subclass fixes its own code: `Sub(message, data)`
    ctor: Any = cls
    return cast(MCPHostError, ctor(message, data))
