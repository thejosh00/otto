#!/bin/sh
# The otto-feedback run's check (README, "otto improving itself"): nothing new while every report
# is handled; a report waiting, or otto failing, is a wake.
if [ "$("$HOME/.local/bin/otto" feedback --json)" = "[]" ]; then
  echo "no feedback waiting"; exit 0
fi
echo "feedback waiting"; exit 1
