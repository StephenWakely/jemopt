#!/usr/bin/env bash

docker network create zorknet

cleanup() {
    echo "Cleaning up..."
    docker network rm "zorknet"
    exit
}

trap cleanup SIGINT

docker run --rm --network zorknet --name faketake datadog/fakeintake

