for v in 24.0.0 25.0.1 26.0.1; do
  echo "=== Testing with mysql $v ==="
  cargo update -p mysql --precise $v
  cargo test --all-features || exit 1
done
