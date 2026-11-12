# First sync

On first start, each index syncs from genesis. On Mainnet or Testnet this takes a while. After a
restart, each index resumes where it stopped.

Zingod serves wallets while it syncs, at the height every index has reached
([heights](../wallets/methods.md#heights)).

## Storage

Keep the indexes on local storage, preferably NVMe. Zingod reads them through mmap, so network
storage performs poorly.

Every index file carries page checksums. [`zingod verify`](maintenance.md#zingod-verify) finds a
damaged index; delete its directory and Zingod resyncs it.

## Index snapshot

A published snapshot fills empty indexes before Zingod starts, which skips most of the first
sync. It needs `aria2c` on `PATH`; the Docker image and the Nix package include it.

```toml
[snapshot]
manifest = "https://snapshots.zingolabs.dev/zingod-snapshot-1.0.0/manifest.json"
connections = 8   # total across all archives, at most 16
```

- Only enabled indexes whose directories are missing or empty are filled. Indexes holding data
  are left alone, and if none are empty nothing is downloaded.
- Free space is checked before downloading. If it is short, startup stops and names the
  directory and the space needed.
- Transient download errors are retried. A restart resumes each archive where it stopped.
- Startup stops on a manifest for another network, a hash mismatch, a permanent download error,
  or an empty index the manifest has no archive for.

While the snapshot installs, the state is `provisioning`, with progress in `detail`
([states](monitoring.md#states)).

The manifest format, for publishing your own snapshot:

```json
{"network": "...", "height": 0,
 "indexes": {"<index>": {"archive": "...", "sha256": "...", "archive_bytes": 0, "installed_bytes": 0}}}
```

All archives come from one node at one `height`. Each is a zstd-compressed tar of the index
directory's contents, with no top-level directory. `archive` is resolved relative to the manifest
URL, and `installed_bytes` is its unpacked size.
