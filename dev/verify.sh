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

run_step "RUSTFLAGS='-D warnings' cargo test" "Rust tests (no features)" "" || exit 1
run_step "RUSTFLAGS='-D warnings' cargo test --features sqlite" "Rust tests (sqlite)" "" || exit 1
run_step "RUSTFLAGS='-D warnings' cargo test --features sqlite,tracing" "Rust tests (sqlite,tracing)" "" || exit 1
run_step "RUSTFLAGS='-D warnings' cargo test --features sqlite,tracing,testing" "Rust tests (sqlite,tracing,testing)" "" || exit 1
run_step "RUSTFLAGS='-D warnings' cargo test --features mysql" "Rust tests (mysql)" "" || exit 1
run_step "RUSTFLAGS='-D warnings' cargo test --features mysql,tracing" "Rust tests (mysql,tracing)" "" || exit 1
run_step "RUSTFLAGS='-D warnings' cargo test --features mysql,tracing,testing" "Rust tests (mysql,tracing,testing)" "" || exit 1
run_step "RUSTFLAGS='-D warnings' cargo test --features sqlite,mysql" "Rust tests (sqlite,mysql)" "" || exit 1
run_step "RUSTFLAGS='-D warnings' cargo test --features sqlite,mysql,tracing" "Rust tests (sqlite,mysql,tracing)" "" || exit 1
run_step "RUSTFLAGS='-D warnings' cargo test --features sqlite,mysql,tracing,testing" "Rust tests (sqlite,mysql,tracing,testing)" "" || exit 1
