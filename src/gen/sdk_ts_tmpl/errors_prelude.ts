/** The `data` object of a host error; every key the host adds is reachable. */
export interface ErrorData {
  error_code?: string;
  argument?: string;
  field?: string;
  expected?: unknown;
  remedy?: string;
  docs?: string;
  help_url?: string;
  request_id?: string;
  [key: string]: unknown;
}

/** Base class of every error the host answers with; `code` is the wire code. */
export class MCPHostError extends Error {
  readonly code: string;
  /** The host's message without the `code: ` prefix. */
  readonly detail: string;
  readonly data: ErrorData;

  constructor(code: string, message: string, data: ErrorData = {}) {
    super(`${code}: ${message}`);
    this.name = new.target.name;
    this.code = code;
    this.data = data;
    this.detail = message;
  }

  get remedy(): string | undefined {
    return this.data.remedy;
  }
}

/** Thrown when the host's contract is not the one this SDK was rendered from (`onSkew: "throw"`). */
export class ContractSkew extends Error {
  readonly local: string;
  readonly host: string;

  constructor(local: string, host: string) {
    super(`contract skew: sdk rendered from ${local}, host serves ${host}`);
    this.name = "ContractSkew";
    this.local = local;
    this.host = host;
  }
}

/** A response did not have the shape the contract promises for `tool`. */
export class ResultShapeError extends Error {
  readonly tool: string;
  readonly path: string;
  readonly reason: string;

  constructor(tool: string, path: string, reason: string) {
    super(`${tool}: result field '${path}': ${reason}`);
    this.name = "ResultShapeError";
    this.tool = tool;
    this.path = path;
    this.reason = reason;
  }
}

/** Build the typed error for a JSON-RPC `error` member (or a tool error body). */
export function errorFromWire(error: { code?: unknown; message?: unknown; data?: unknown }): MCPHostError {
  const raw = error.data;
  const data: ErrorData = raw !== null && typeof raw === "object" && !Array.isArray(raw) ? { ...(raw as ErrorData) } : {};
  let code: string | undefined = typeof data.error_code === "string" ? data.error_code : undefined;
  if (code === undefined && typeof (data as { code?: unknown }).code === "string") {
    code = (data as { code: string }).code;
  }
  if (code === undefined) {
    code = typeof error.code === "string" ? error.code : "unknown";
  }
  const message = typeof error.message === "string" ? error.message : "";
  const Cls = ERRORS_BY_CODE[code];
  if (Cls === undefined) {
    return new MCPHostError(code, message, data);
  }
  return new Cls(message, data);
}
