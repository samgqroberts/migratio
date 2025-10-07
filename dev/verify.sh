#!/bin/bash

## This script runs the suite of all verifications

set +e

SCRIPT_DIR="$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"
BASE_DIR="`dirname "$SCRIPT_DIR"`"

function run_step() {
  local cmd="$1"
  local description="$2"
  local dir="$3"

  printf "Running $description... "

  # Capture output and exit status
  output=$(cd "$BASE_DIR/$dir" && eval "$cmd" 2>&1)
  exit_code=$?

  if [ $exit_code -ne 0 ]; then
    echo "❌"
    echo ""
    echo "$output"
    return $exit_code
  fi

  echo "✅"
  return 0
}

# Main execution
echo "Code verification"
echo "======================"

run_step "RUSTFLAGS='-D warnings' cargo test --all-features" "Rust tests" "" || exit 1

run_step "RUSTFLAGS='-D warnings' cargo test --doc" "Doctests with no features" "" || exit 1
