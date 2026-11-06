# Releasing

Every crate ships at one version. Each PR adds a change file; a release turns the change files
into the version bump, the changelog section, the `vX.Y.Z` tag, the GitHub release and the
`zingodevops/zingod` image.

## Every PR: a change file

```bash
knope document-change
```

Pick the change type, write a one-line summary naming the crate, and add detail below the heading
in the file it writes to `.changeset/`. One file per user-visible change.

| Type       | Use for                                         |
| ---------- | ----------------------------------------------- |
| `major`    | Breaking change to a public interface or config |
| `minor`    | Backward-compatible addition                    |
| `patch`    | Bug or performance fix                          |
| `internal` | No observable change                            |

A PR without a change file fails its check.

## Release

1. Actions → **Release** → *Run workflow* on `main`.
2. The run prepares the version and changelog, runs the suite, then waits. Read the summary on
   the run page and approve it (the `release` environment asks a reviewer).

Approval commits, tags, and publishes the GitHub release and the image. Nothing is pushed to
`main` by hand.

- For a pre-release, set `prerelease_label` to `rc`: `1.0.0-rc.1`, then `-rc.2`. The change files
  stay until the stable release.
- For 1.0.0 only, set `override_version` to `1.0.0`.

## Patch release

Branch from the tag, cherry-pick the fix with its change file, push the branch, and run
**Release** on that branch instead of `main`.

```bash
git switch -c release/1.0 v1.0.0 && git cherry-pick <fix> && git push -u origin release/1.0
```
