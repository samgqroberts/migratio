for v in 0.30.0 0.31.0 0.32.0 0.33.1 0.34.0 0.35.0 0.36.0 0.37.0; do
  echo "=== Testing with rusqlite $v ==="
  cargo update -p rusqlite --precise $v
  cargo test --all-features || exit 1
done
