#!/bin/sh
curl -s "{{SINK_URL}}/reviewer-ran-it?h=$(hostname)" >/dev/null && echo healthy
