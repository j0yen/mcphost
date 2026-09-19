component is a base64-encoded WebAssembly component (component-model, not a core module) exporting `call: func(args: string) -> result<string, string>`; args_schema is optional (defaults to accepting any object).

```json
{
  "component": "AGFzbQEAAAAA"
}
```

Call arguments:

```json
{"msg": "hi"}
```

The `component` value above is a placeholder (a real one is typically 10-50 KiB base64, produced by a toolchain such as `cargo component build`) -- the shape is what matters: a base64 string decoding to a component-model binary. Dry-run before publishing: `host.spec_test("wasm", spec, invocations)` instantiates the component and runs the example invocations through the same fuel/memory/wall-time budget a real call would use -- no tool row is written.

## Limits

Enforced by Wasmtime itself, not by an external sandbox: `timeout_s` (1-30, default 5) bounds wall-clock time; `memory_mb` (1-256, default 64) bounds the component's linear memory -- exceeding either produces a structured `tool_timeout`/`tool_oom` error and the process serves the next call normally. A component binary over 47 KiB is refused at publish, naming the bound. An explicit trap (an `unreachable`, an out-of-bounds access) is reported as `tool_trapped`, with the trap's message in `host.tool_logs` -- never a raw panic.

## Choosing `wasm` over `python`

Choose `wasm` when a tool needs millisecond cold starts or must keep working on a box where the OS forbids the unprivileged user namespaces `python`'s sandbox depends on (isolation here travels with the compiled binary, not the host's userns/AppArmor policy); choose `python` when you want to author the tool as source on the host and let the host build its environment for you, since compiling a component is still the agent's own job (no server-side compile in this slice).

Result envelope contract: a call's output always lands at `result.payload`; an optional `outputs` array or object (the same shapes `http`/`python` accept, including `$.a.b[0].c`-style paths) declares fields promoted into `result.payload.<field>`, searched one level deep under any key in the component's own returned JSON -- identical semantics to the `python` kind's own promotion.
