# Denju CLI structured output v1

Commands intended for automation accept `--json` and write exactly one JSON value followed by one newline to stdout. JSON mode never opens an interactive prompt. Progress and diagnostics must not contaminate stdout.

Success shape:

```json
{
  "version": 1,
  "ok": true,
  "result": {}
}
```

Failure shape:

```json
{
  "version": 1,
  "ok": false,
  "error": {
    "code": "invalid_arguments",
    "message": "human-readable explanation",
    "recovery": "denju --help"
  }
}
```

`result` is present only on success and `error` only on failure. `error.code` is the stable machine value; `message` and optional `recovery` are explanatory. Exit status remains meaningful independently of the JSON envelope.

The Rust source of truth is `denju-wire::CliEnvelope`; its unit tests freeze the serialized v1 field names and success/failure exclusivity.
