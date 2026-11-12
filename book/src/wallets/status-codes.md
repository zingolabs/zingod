# Status codes

How to handle each gRPC status Zingod returns. A rejected `SendTransaction` is not an error
status; it is reported in the response ([details](methods.md#transactions)).

| Code                 | When                                                                                                  | Client action                     |
| -------------------- | ----------------------------------------------------------------------------------------------------- | --------------------------------- |
| `UNAVAILABLE`        | the server is starting, at capacity, or cannot reach its data sources; a stream had nothing to send for 5 minutes | retry with backoff                |
| `NOT_FOUND`          | the height, hash or txid is not in the chain, or is above the index's tip                             | do not retry the same request     |
| `OUT_OF_RANGE`       | `GetBlockRange` reached the tip, after sending every block up to it                                   | retry once the tip advances       |
| `INVALID_ARGUMENT`   | malformed request, height above the protocol maximum, invalid or wrong-network address, invalid `poolTypes` | fix the request                   |
| `RESOURCE_EXHAUSTED` | request body too large (64 KiB; 2 MB + 1 KiB for `SendTransaction`), or too many address outputs in one request | send less per request             |
| `DEADLINE_EXCEEDED`  | the request's `grpc-timeout` passed, or the request body took more than 30 seconds to arrive          | retry with a longer deadline      |
| `UNIMPLEMENTED`      | the method or `shieldedProtocol` is unsupported, or its index is disabled                             | do not retry                      |
| `PERMISSION_DENIED`  | the browser origin is blocked by the operator                                                         | do not retry                      |
| `INTERNAL`           | server fault                                                                                          | report to the operator            |

When the server is at capacity, the `UNAVAILABLE` response carries `grpc-retry-pushback-ms`
(250). Wait at least that long before retrying.
