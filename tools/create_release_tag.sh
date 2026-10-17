#!/usr/bin/env bash
# Called only from the main-branch release workflow; never suppress push errors.
set -euo pipefail
: "${TAG:?TAG is required}"
: "${RELEASE_COMMIT:?RELEASE_COMMIT is required}"
: "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"

if git show-ref --verify --quiet "refs/tags/$TAG"; then
    if [ "$(git rev-parse "$TAG^{commit}")" = "$RELEASE_COMMIT" ]; then
        # A prior run may have pushed successfully before a build failed.
        echo "Tag $TAG identifies this commit; continue or retry its release."
        echo "publish=true" >> "$GITHUB_OUTPUT"
    else
        echo "Tag $TAG belongs to an earlier commit; no new release requested."
        echo "publish=false" >> "$GITHUB_OUTPUT"
    fi
    exit 0
fi
git config user.name 'github-actions[bot]'
git config user.email 'github-actions[bot]@users.noreply.github.com'
git tag -a "$TAG" "$RELEASE_COMMIT" -m "Release $TAG"
if ! git push origin "refs/tags/$TAG"; then
    # Only an identical remote commit is an acceptable concurrent push.
    # Authentication/network failures and conflicting tags must fail.
    git fetch origin "refs/tags/$TAG"
    test "$(git rev-parse 'FETCH_HEAD^{commit}')" = "$RELEASE_COMMIT"
fi
echo "publish=true" >> "$GITHUB_OUTPUT"
