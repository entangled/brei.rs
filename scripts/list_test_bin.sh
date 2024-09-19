run_test() {
    RUSTFLAGS="-C instrument-coverage" \
    cargo test --no-run --message-format=json \
        | jq -r "select(.profile.test == true) | .filenames[]" \
        | grep -v dSYM -
}

for file in $( run_test ); do
    printf "%s %s " -object $file; \
done
