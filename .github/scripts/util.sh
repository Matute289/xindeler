#!/bin/sh

### Returns the Docker tag to publish.
### release-tag => <release-tag> (e.g. v1.2.3)
### schedule    => nightly
### main push   => master
### else        => ""
publishdockertag() {
  export PUBLISH_DOCKER_TAG="";

  # GitHub Actions uses GITHUB_REF_TYPE=tag and GITHUB_REF_NAME=v1.2.3
  TAG_REGEX='^v[0-9]+\.[0-9]+\.[0-9]+$'
  if [ "${GITHUB_REF_TYPE}" = "tag" ] && echo "${GITHUB_REF_NAME}" | grep -Eq "${TAG_REGEX}"; then
    export PUBLISH_DOCKER_TAG="${GITHUB_REF_NAME}";
    return 0
  fi

  # Schedule event
  if [ "${GITHUB_EVENT_NAME}" = "schedule" ]; then
    export PUBLISH_DOCKER_TAG="nightly";
    return 0;
  fi

  # Push to main branch
  if [ "${GITHUB_EVENT_NAME}" = "push" ] && [ "${GITHUB_REF_NAME}" = "main" ]; then
    export PUBLISH_DOCKER_TAG="master";
    return 0;
  fi
}
