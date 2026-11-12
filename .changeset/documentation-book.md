---
default: patch
---

# Documentation moves to a book at zingolabs.github.io/zingod

All documentation outside the crates now lives in one book, with a part each for wallet
developers (connecting, each method's answers, status codes, the protocol files), operators
(quick start, configuration, first sync, monitoring, maintenance, network exposure, Docker,
migrating from Zaino) and contributors (architecture, testing). The `docs/` directory is gone;
the example configs are in `book/src/operators/`.
