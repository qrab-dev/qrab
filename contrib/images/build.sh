#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
# The images, one per line.
components=(
  manager
)
component=${1:?usage: build.sh $(IFS='|'; printf '%s' "${components[*]}") TAG}
tag=${2:?image tag required}
[[ $tag =~ ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$ ]] || { echo 'invalid image tag' >&2; exit 1; }
revision=$(git rev-parse HEAD)
# Site settings: QRAB_REGISTRY (required) is the registry host[/prefix] the
# images are tagged for; QRAB_SOURCE_URL (optional) labels them with the
# source repository. A site can keep its values in an env file and source it.
registry=${QRAB_REGISTRY:?set QRAB_REGISTRY to the image registry, e.g. registry.example.org}
source_url=${QRAB_SOURCE_URL:-}
image="$registry/qrab/$component:$tag"
case "$component" in
  manager)
    podman build --label "org.opencontainers.image.revision=$revision" -f contrib/images/Containerfile.manager --build-arg "QRAB_REVISION=$revision" --build-arg "QRAB_SOURCE_URL=$source_url" \
      -t "$image" .
    ;;
  *) echo "component must be one of: ${components[*]}" >&2; exit 1;;
esac
printf '%s\n' "$image"
