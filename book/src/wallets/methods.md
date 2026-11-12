# Methods

The [protocol files](protocol.md) define every method. This page covers only behaviour the
protocol leaves open, and the places where Zingod differs from lightwalletd.

## Heights

The **served tip** is the block Zingod currently answers at. Every index holds it.

- Methods that take no height answer at the served tip: `GetLatestBlock`, `GetLatestTreeState`,
  `GetSubtreeRoots`, the address methods, and `GetLightdInfo.blockHeight`. A wallet that only
  asks for heights up to `GetLatestBlock` always gets an answer.
- Methods that take a height (`GetBlock`, `GetBlockRange`, `GetTreeState`) answer up to the tip of
  the index they read. While the server is syncing, that can be above the served tip.
- If a `BlockID` has both a height and a hash, the hash is used.
- Unmined transactions are reported at height 0.

## Blocks

**`GetBlockRange`** has no limit on range size, and it never returns fewer blocks than asked
for without an error. If the range extends past the tip, the stream sends every block up to the
tip and then ends with `OUT_OF_RANGE`. If the range starts past the tip, it fails with
`OUT_OF_RANGE` before sending anything.

**`GetBlock`** returns every pool, including transparent inputs and outputs, as lightwalletd
does. It takes no `poolTypes`.

**`CompactTx.fee`** is filled in for mined and mempool transactions. lightwalletd leaves it 0.
Zingod reports 0 for coinbase transactions, for a wallet's own transaction before another node
lists it, and for fees of 2^32 zatoshis or more, which do not fit the `uint32` field.

## Tree state

**`GetTreeState`** answers for exactly the requested height. The block hash is in display order.
Each tree field holds a hex-encoded `CommitmentTree`:

- `""` if the pool is not active at that height.
- `"000000"` if the pool is active but empty.

**`GetSubtreeRoots`** either streams the whole answer or fails before the first message. A
`startIndex` past the last root returns an empty stream. `maxEntries` 0 returns every root. An
unknown `shieldedProtocol` fails with `UNIMPLEMENTED`.

## Transactions

**`GetTransaction`** accepts only a txid (`TxFilter.hash`). A `(block, index)` filter fails with
`INVALID_ARGUMENT`. Zingod asks its validators for the transaction and returns `NOT_FOUND` only
when every validator asked reports it missing. If no validator responds, Zingod serves the
transaction from its mempool or fetches its block from p2p peers; if neither works, the call fails
with `UNAVAILABLE`.

**`SendTransaction`** reports a rejected transaction in the response, as lightwalletd does. The
call returns `OK` with:

| `errorCode` | Meaning                                                                       |
| ----------- | ----------------------------------------------------------------------------- |
| `0`         | accepted; `errorMessage` is the txid, quoted, in display order                |
| `-22`       | the transaction could not be parsed                                           |
| `-25`       | the transaction has expired, or targets the wrong consensus branch            |
| other       | the validator rejected it; `errorMessage` is the validator's message verbatim |

The call itself fails with `UNAVAILABLE` only when no validator accepted the transaction and at
least one could not be reached. Retry in that case.

## Mempool

The mempool Zingod serves contains transactions listed by a trusted validator, transactions a
quorum of its p2p peers agree on, and transactions submitted through this server. Both mempool
methods fail with `UNAVAILABLE` when no trusted validator at the best block is reachable and
Zingod has too few p2p peers for a quorum.

**`GetMempoolStream`** starts with the current mempool, then sends each new transaction once,
each with height 0. The stream ends with `OK` when a new block arrives. By then the server already
serves that block, so a `GetLatestBlock` sent next returns it.

**`GetMempoolTx`** accepts `poolTypes` like `GetBlockRange`.

## Transparent addresses

The address methods (`GetAddressUtxos`, `GetAddressUtxosStream`, `GetTaddressBalance`,
`GetTaddressBalanceStream`, `GetTaddressTransactions`) answer at the served tip.

- A request covering more than 100,000 received outputs (the default limit) fails with
  `RESOURCE_EXHAUSTED`. Send fewer addresses per request.
- An address for another network fails with `INVALID_ARGUMENT`.
- Do not page `GetAddressUtxos` by `startHeight`. It is not a unique resume key and paging can
  stall ([lightwallet-protocol#33](https://github.com/zcash/lightwallet-protocol/issues/33)).

Address queries tell the server which addresses belong to one wallet. To find transparent
activity privately, request the `TRANSPARENT` pool in `GetBlockRange` and match scripts locally.
Coinbase inputs are omitted (a coinbase is the transaction at `CompactTx.index` 0), and
`CompactTxIn` carries no value, so balances still require the address methods.

## Fees

**`GetStandardFee`** returns the ZIP 317 marginal fee per logical action, in zatoshis, for the
next block. The estimator version is 0.

## Pool filtering

`poolTypes` on `GetBlockRange` and `GetMempoolTx` selects the pools returned:

- Empty: Sapling, Orchard and Ironwood. Transparent data is excluded.
- Otherwise: exactly the listed pools.
- `POOL_TYPE_INVALID` or an unknown value fails with `INVALID_ARGUMENT`.

`GetBlockRangeNullifiers` ignores `TRANSPARENT`, as the protocol specifies.

## Deprecated and unsupported

- `GetBlockRangeNullifiers` and `GetTaddressTxids` are deprecated and will be removed. Use
  `GetBlockRange` with shielded `poolTypes`, and `GetTaddressTransactions`.
- `GetBlockNullifiers` and `Ping` are not supported and return `UNIMPLEMENTED`.
- Operators can disable individual indexes. Methods that depend on a disabled index return
  `UNIMPLEMENTED`. `/status` lists the methods each enabled index serves.
