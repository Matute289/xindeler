#!/bin/bash
XINDELER_ASSETS="$(pwd)/assets";
export XINDELER_ASSETS;

time cargo test;
