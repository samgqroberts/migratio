# Run all verification steps
verify: test-no-features test-sqlite test-sqlite-tracing test-sqlite-tracing-testing test-mysql test-mysql-tracing test-mysql-tracing-testing test-sqlite-mysql test-sqlite-mysql-tracing test-sqlite-mysql-tracing-testing

# Run tests with no features
test-no-features:
    RUSTFLAGS='-D warnings' cargo test

# Run tests with sqlite feature
test-sqlite:
    RUSTFLAGS='-D warnings' cargo test --features sqlite

# Run tests with sqlite,tracing features
test-sqlite-tracing:
    RUSTFLAGS='-D warnings' cargo test --features sqlite,tracing

# Run tests with sqlite,tracing,testing features
test-sqlite-tracing-testing:
    RUSTFLAGS='-D warnings' cargo test --features sqlite,tracing,testing

# Run tests with mysql feature
test-mysql:
    RUSTFLAGS='-D warnings' cargo test --features mysql

# Run tests with mysql,tracing features
test-mysql-tracing:
    RUSTFLAGS='-D warnings' cargo test --features mysql,tracing

# Run tests with mysql,tracing,testing features
test-mysql-tracing-testing:
    RUSTFLAGS='-D warnings' cargo test --features mysql,tracing,testing

# Run tests with sqlite,mysql features
test-sqlite-mysql:
    RUSTFLAGS='-D warnings' cargo test --features sqlite,mysql

# Run tests with sqlite,mysql,tracing features
test-sqlite-mysql-tracing:
    RUSTFLAGS='-D warnings' cargo test --features sqlite,mysql,tracing

# Run tests with sqlite,mysql,tracing,testing features
test-sqlite-mysql-tracing-testing:
    RUSTFLAGS='-D warnings' cargo test --features sqlite,mysql,tracing,testing

# Verify compatibility with multiple mysql crate versions
verify-mysql-versions:
    #!/usr/bin/env bash
    for v in 24.0.0 25.0.1 26.0.1; do
      echo "=== Testing with mysql $v ==="
      cargo update -p mysql --precise $v
      cargo test --all-features || exit 1
    done

# Verify compatibility with multiple rusqlite crate versions
verify-sqlite-versions:
    #!/usr/bin/env bash
    for v in 0.30.0 0.31.0 0.32.0 0.33.1 0.34.0 0.35.0 0.36.0 0.37.0; do
      echo "=== Testing with rusqlite $v ==="
      cargo update -p rusqlite --precise $v
      cargo test --all-features || exit 1
    done
