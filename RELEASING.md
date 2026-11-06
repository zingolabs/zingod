# Releasing

Every crate shares one version (`[workspace.package] version` in `Cargo.toml`), and
[Knope](https://knope.tech) (`knope.toml`, in the nix devshell) does the release work. Each PR
records its changes as change files in `.changeset/`. A release turns them into the version bump,
the `CHANGELOG.md` section, the GitHub release and the `vX.Y.Z` tag. Pushing the tag runs the one
release workflow, `.github/workflows/release.yaml`, which builds the `zingod` image and pushes it to
Docker Hub as `zingodevops/zingod:X.Y.Z`, plus `zingodevops/zingod:latest` unless it is a
pre-release.

## Every PR: a change file

```bash
knope document-change
```

It asks for the change type and a one-line summary, and writes `.changeset/<summary>.md`. Add
detail below the heading. Write one file per user-visible change, and name the crate in the
summary:

```markdown
---
default: major
---

# `zingod-grpc`: `GetLightdInfo.version` is the daemon's version

Pass it in `RouteDeps.version`.
```

| Type       | Use for                                                        | Version bump |
| ---------- | -------------------------------------------------------------- | ------------ |
| `major`    | Breaking change to a public interface, config or wire contract | major        |
| `minor`    | Backward-compatible addition                                   | minor        |
| `patch`    | Bug or performance fix                                         | patch        |
| `internal` | No observable change (listed under Internal)                   | patch        |

The [Knope Bot](https://github.com/marketplace/knope-bot) GitHub App fails a PR that has no
change file.

## Release

From the devshell, with a GitHub token that can create releases on `zingolabs/zingod`. Every
command takes `--dry-run`: run it that way first.

```bash
# 1. newest main commit whose CI passed
SHA=$(gh run list -w ci.yml -b main -s success -L1 --json headSha -q '.[0].headSha')

# 2. release branch at that commit
git fetch origin && git switch -c release/1.0 "$SHA" && git push -u origin release/1.0

# 3. bump + CHANGELOG.md from .changeset/, commit, push, GitHub release + tag v1.0.0
GITHUB_TOKEN=$(gh auth token) knope release

# 4. carry the bump and the consumed change files back to main (needs one approval)
gh pr create -B main -f && gh pr merge --auto --merge
```

Step 4 is required. Without it `main` keeps the old version and the change files that were
already released.

A pre-release takes a label: `knope release --prerelease-label rc` gives `1.0.0-rc.1`, then
`1.0.0-rc.2`. A pre-release keeps the change files; the stable release consumes them. Leaving the
label out releases `1.0.0`. To release a version other than the one the change files add up to,
pass `--override-version X.Y.Z`.

## Patch release

On the release branch, cherry-pick the fix together with its change file, run `knope release`,
then step 4.
